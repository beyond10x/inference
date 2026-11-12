//! The adapter reference, `website/docs/reference/adapters.md`: which capabilities each model
//! binding claims, from the records the adapters commit, and what the local ones measured on the
//! reference machine.
//!
//! This crate links no adapter. A hosted binding's claims are a claims record under the adapter's
//! `records/`, held equal to the binding's code by a test in that adapter (`held_by`). A local
//! binding's claims are the capabilities its capability record gives evidence for, which the
//! adapter's record test loads and checks. Generation refuses a record whose `adapter_revision` is
//! not the workspace version, a claim that is not a capability, a feature the package does not
//! have, and a test the record names that its adapter does not define.

use std::fs;
use std::path::Path;

use inference_core::model::Capability;
use serde::Deserialize;
use serde_json::Value;

use crate::crates::{Package, packages, workspace_version};
use crate::{Result, SOURCE, TREE, cell, front_matter, test_defined};

/// Where the adapter reference lands, relative to the repository root.
pub const PAGE: &str = "website/docs/reference/adapters.md";

/// Where a binding's claims come from.
enum Record {
    /// A claims record of a hosted binding, under the adapter's directory.
    Claims(&'static str),
    /// A capability record of a local binding, under the adapter's directory, and the model it
    /// was measured with.
    Capability(&'static str, &'static str),
}

/// One model binding the page lists.
struct Binding {
    /// What the reader calls it.
    name: &'static str,
    /// The workspace package that holds it.
    package: &'static str,
    /// The contract it implements and how it runs.
    runs: &'static str,
    /// The features a local binding needs; a hosted binding's come from its record.
    features: &'static [&'static str],
    record: Record,
    /// The variables it reads.
    environment: &'static str,
    /// The licences of its code and weights, or where its data goes.
    licence: &'static str,
    /// The release whose `CHANGELOG.md` section introduced it, `0.x.0`.
    since: &'static str,
}

/// Every model binding, in the order the page lists them.
const BINDINGS: &[Binding] = &[
    Binding {
        name: "OpenAI Realtime",
        package: "inference-openai",
        runs: "native speech (duplex); hosted, WebSocket",
        features: &[],
        record: Record::Claims("records/claims-realtime.json"),
        environment: "`OPENAI_API_KEY`, or the keychain secret `openai-api-key` with feature `keychain`; `INFERENCE_OPENAI_REALTIME_MODEL`",
        licence: "hosted service: audio leaves the machine",
        since: "0.5.0",
    },
    Binding {
        name: "OpenAI GPT-Live",
        package: "inference-openai",
        runs: "native speech (duplex); hosted, WebSocket (`v1/live/sessions`)",
        features: &[],
        record: Record::Claims("records/claims-gpt-live.json"),
        environment: "`OPENAI_API_KEY`, or the keychain secret `openai-api-key` with feature `keychain`",
        licence: "hosted service: audio leaves the machine; billed per session second",
        since: "0.6.0",
    },
    Binding {
        name: "Amazon Nova Sonic on Bedrock",
        package: "inference-aws",
        runs: "native speech (duplex); hosted, bidirectional stream",
        features: &[],
        record: Record::Claims("records/claims-nova-sonic.json"),
        environment: "the AWS default credential chain; `AWS_REGION`; `INFERENCE_NOVA_SONIC_MODEL`",
        licence: "hosted service: audio leaves the machine",
        since: "0.5.0",
    },
    Binding {
        name: "ElevenLabs streaming TTS",
        package: "inference-elevenlabs",
        runs: "TTS, streaming; hosted, WebSocket (`multi-stream-input`), one connection per utterance, one context per segment",
        features: &[],
        record: Record::Claims("records/claims-tts.json"),
        environment: "`ELEVENLABS_API_KEY`, or the keychain secret `elevenlabs-api-key` with feature `keychain`",
        licence: "hosted service: text leaves the machine",
        since: "0.16.0",
    },
    Binding {
        name: "OpenAI streamed speech TTS",
        package: "inference-openai",
        runs: "TTS, streaming; hosted, HTTP (`POST /v1/audio/speech`, chunked PCM), one request per segment",
        features: &[],
        record: Record::Claims("records/claims-speech.json"),
        environment: "`OPENAI_API_KEY`, or the keychain secret `openai-api-key` with feature `keychain`",
        licence: "hosted service: text leaves the machine",
        since: "0.22.0",
    },
    Binding {
        name: "ElevenLabs agent conversation",
        package: "inference-elevenlabs",
        runs: "native speech (duplex); hosted, WebSocket (`v1/convai/conversation`), the agent's own model, voice and formats",
        features: &[],
        record: Record::Claims("records/claims-agent.json"),
        environment: "`ELEVENLABS_AGENT_ID` for a public agent, or `ELEVENLABS_AGENT_SIGNED_URL` for a private one (the live test and `compare`)",
        licence: "hosted service: audio leaves the machine",
        since: "0.17.0",
    },
    Binding {
        name: "Cartesia Sonic streaming TTS",
        package: "inference-cartesia",
        runs: "TTS, streaming; hosted, WebSocket (`tts/websocket`), one connection per synthesis, one context per segment",
        features: &[],
        record: Record::Claims("records/claims-tts.json"),
        environment: "`CARTESIA_API_KEY`, or the keychain secret `cartesia-api-key` with feature `keychain`",
        licence: "hosted service: text leaves the machine",
        since: "0.22.0",
    },
    Binding {
        name: "Deepgram streaming STT",
        package: "inference-deepgram",
        runs: "STT, streaming; hosted, WebSocket (`v1/listen`), one connection per recognition",
        features: &[],
        record: Record::Claims("records/claims-stt.json"),
        environment: "`DEEPGRAM_API_KEY`, or the keychain secret `deepgram-api-key` with feature `keychain`",
        licence: "hosted service: audio leaves the machine",
        since: "0.17.0",
    },
    Binding {
        name: "ElevenLabs realtime STT",
        package: "inference-elevenlabs",
        runs: "STT, streaming; hosted, WebSocket (`v1/speech-to-text/realtime`), one connection per recognition, μ-law 8 kHz or PCM16 in, committed spans with word timings",
        features: &[],
        record: Record::Claims("records/claims-stt.json"),
        environment: "`ELEVENLABS_API_KEY`, or the keychain secret `elevenlabs-api-key` with feature `keychain`",
        licence: "hosted service: audio leaves the machine",
        since: "0.28.0",
    },
    Binding {
        name: "Deepgram Voice Agent",
        package: "inference-deepgram",
        runs: "native speech (duplex); hosted, WebSocket (`v1/agent/converse`), the listen, think and speak providers its `Settings` name",
        features: &[],
        record: Record::Claims("records/claims-voice-agent.json"),
        environment: "`DEEPGRAM_API_KEY` with Voice Agent access, or the keychain secret `deepgram-api-key` with feature `keychain`",
        licence: "hosted service: audio leaves the machine",
        since: "0.17.0",
    },
    Binding {
        name: "Phonic speech-to-speech",
        package: "inference-phonic",
        runs: "native speech (duplex); hosted, WebSocket (`v1/sts/ws`), `phonic-1` unless configured otherwise",
        features: &[],
        record: Record::Claims("records/claims-duplex.json"),
        environment: "`PHONIC_API_KEY`, or the keychain secret `phonic-api-key` with feature `keychain`",
        licence: "hosted service: audio leaves the machine",
        since: "0.22.0",
    },
    Binding {
        name: "Deepslate Opal",
        package: "inference-deepslate",
        runs: "native speech (duplex); hosted, WebSocket (`api/v1/vendors/<vendor>/organizations/<organization>/realtime`), protocol buffers messages",
        features: &[],
        record: Record::Claims("records/claims-opal.json"),
        environment: "`DEEPSLATE_API_KEY`, or the keychain secret `deepslate-api-key` with feature `keychain`; `DEEPSLATE_VENDOR_ID`, `DEEPSLATE_ORGANIZATION_ID`, and `DEEPSLATE_VOICE_ID` for spoken output",
        licence: "hosted service: audio leaves the machine",
        since: "0.22.0",
    },
    Binding {
        name: "sherpa-onnx streaming STT",
        package: "inference-sherpa-onnx",
        runs: "STT, streaming; in process, native library",
        features: &["stt"],
        record: Record::Capability(
            "records/stt-i9-10900k-cpu.json",
            "streaming Zipformer, English",
        ),
        environment: "`INFERENCE_SHERPA_ONNX_STT_DIR`",
        licence: "code Apache-2.0 (sherpa-onnx 1.13.8), MIT (onnxruntime); weights Apache-2.0, trained on LibriSpeech (CC BY 4.0)",
        since: "0.6.0",
    },
    Binding {
        name: "whisper.cpp batch STT",
        package: "inference-whispercpp",
        runs: "STT, batch; in process, native library, CPU",
        features: &[],
        record: Record::Capability("records/stt-i9-10900k-cpu.json", "Whisper base.en"),
        environment: "`INFERENCE_WHISPERCPP_STT_MODEL`",
        licence: "code MIT (whisper.cpp), Unlicense (whisper-rs); weights MIT",
        since: "0.6.0",
    },
    Binding {
        name: "llama-server LLM",
        package: "inference-llama-server",
        runs: "LLM; local worker, loopback HTTP",
        features: &[],
        record: Record::Capability(
            "capability/reference-machine-cpu.json",
            "Qwen2.5-1.5B-Instruct q4_k_m",
        ),
        environment: "`INFERENCE_LLAMA_SERVER_URL`, `INFERENCE_LLAMA_SERVER_MODEL`",
        licence: "code MIT (llama.cpp); weights Apache-2.0",
        since: "0.6.0",
    },
    Binding {
        name: "llama-server LLM, tools on",
        package: "inference-llama-server",
        runs: "LLM; local worker, loopback HTTP, server started with `--jinja`",
        features: &["tools"],
        record: Record::Capability(
            "capability/reference-machine-cpu-tools.json",
            "Qwen2.5-1.5B-Instruct q4_k_m",
        ),
        environment: "`INFERENCE_LLAMA_SERVER_URL`, `INFERENCE_LLAMA_SERVER_MODEL`",
        licence: "code MIT (llama.cpp); weights Apache-2.0",
        since: "0.6.0",
    },
];

/// A hosted binding's claims record.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimsRecord {
    model: String,
    binding: String,
    features: Vec<String>,
    claims: Vec<String>,
    held_by: String,
    adapter_revision: String,
}

/// The parts of a local capability record the page reads.
#[derive(Deserialize)]
struct CapabilityRecord {
    hardware: String,
    evidence: Vec<Evidence>,
    resource_profile: ResourceProfile,
    adapter_revision: String,
}

#[derive(Deserialize)]
struct Evidence {
    capability: String,
    tests: Vec<String>,
}

#[derive(Deserialize)]
struct ResourceProfile {
    peak_resident_memory_mib: u64,
    load_time_ms: u64,
    #[serde(default)]
    cpu_real_time_factor_milli: Option<u64>,
    gpu_memory_mib: Option<u64>,
}

/// One binding with what its record says.
struct Read<'a> {
    binding: &'a Binding,
    model: String,
    features: Vec<String>,
    claims: Vec<String>,
    /// The record's path relative to the repository root.
    path: String,
    /// Local only: the hardware and the resource profile.
    measured: Option<(String, ResourceProfile)>,
}

/// The serialized name of every capability, in declaration order.
fn capability_names() -> Result<Vec<String>> {
    Capability::ALL
        .iter()
        .map(|capability| match serde_json::to_value(capability) {
            Ok(Value::String(name)) => Ok(name),
            _ => Err(format!("{capability:?} does not serialize as a name")),
        })
        .collect()
}

/// Refuse `test` (`binary::function`) unless `tests/<binary>.rs` of the package defines it as a
/// test. An ignored test is admitted: a local record names tests that need a checkpoint or a
/// running server, which the adapter's record test runs when they are present.
fn check_test(root: &Path, package: &Package, test: &str, what: &str) -> Result<()> {
    let (binary, name) = test
        .split_once("::")
        .ok_or_else(|| format!("{what}: {test} is not `binary::function`"))?;
    let file = format!("{}/tests/{binary}.rs", package.directory);
    let source = fs::read_to_string(root.join(&file))
        .map_err(|error| format!("{what}: reading {file}: {error}"))?;
    match test_defined(&source, name) {
        Ok(()) | Err("an ignored test") => Ok(()),
        Err(problem) => Err(format!("{what}: {file} has no test {name} ({problem})")),
    }
}

fn read<'a>(
    root: &Path,
    binding: &'a Binding,
    package: &Package,
    version: &str,
    names: &[String],
) -> Result<Read<'a>> {
    let relative = match binding.record {
        Record::Claims(path) | Record::Capability(path, _) => {
            format!("{}/{path}", package.directory)
        }
    };
    let text = fs::read_to_string(root.join(&relative))
        .map_err(|error| format!("{}: reading {relative}: {error}", binding.name))?;
    let parse_error = |error: serde_json::Error| format!("{}: {relative}: {error}", binding.name);
    let (model, features, claims, revision, measured) = match binding.record {
        Record::Claims(_) => {
            let record: ClaimsRecord = serde_json::from_str(&text).map_err(parse_error)?;
            if !record.binding.starts_with(&package.name.replace('-', "_")) {
                return Err(format!(
                    "{}: {relative} names binding {} outside {}",
                    binding.name, record.binding, package.name
                ));
            }
            check_test(root, package, &record.held_by, binding.name)?;
            (
                record.model,
                record.features,
                record.claims,
                record.adapter_revision,
                None,
            )
        }
        Record::Capability(_, model) => {
            let record: CapabilityRecord = serde_json::from_str(&text).map_err(parse_error)?;
            for evidence in &record.evidence {
                if evidence.tests.is_empty() {
                    return Err(format!(
                        "{}: {relative} claims {} with no test",
                        binding.name, evidence.capability
                    ));
                }
                for test in &evidence.tests {
                    check_test(root, package, test, binding.name)?;
                }
            }
            (
                model.to_owned(),
                binding.features.iter().map(|f| (*f).to_owned()).collect(),
                record
                    .evidence
                    .iter()
                    .map(|evidence| evidence.capability.clone())
                    .collect(),
                record.adapter_revision,
                Some((record.hardware, record.resource_profile)),
            )
        }
    };
    if revision != version {
        return Err(format!(
            "{}: {relative} is of adapter revision {revision}, the workspace is at {version}",
            binding.name
        ));
    }
    for claim in &claims {
        if !names.contains(claim) {
            return Err(format!(
                "{}: {relative} claims {claim}, which is not a capability",
                binding.name
            ));
        }
    }
    for feature in &features {
        if !package.features.contains(feature) {
            return Err(format!(
                "{}: {} has no feature {feature}",
                binding.name, package.name
            ));
        }
    }
    Ok(Read {
        binding,
        model,
        features,
        claims,
        path: relative,
        measured,
    })
}

fn code_list(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_owned()
    } else {
        items
            .iter()
            .map(|item| format!("`{item}`"))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// Thousands separated by commas.
fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn page_for(root: &Path, metadata: &Value, bindings: &[Binding]) -> Result<String> {
    let all = packages(metadata)?;
    let version = workspace_version(metadata)?;
    let names = capability_names()?;
    let mut reads = Vec::new();
    for binding in bindings {
        let package = all
            .iter()
            .find(|package| package.name == binding.package)
            .ok_or_else(|| format!("{}: no package {}", binding.name, binding.package))?;
        reads.push(read(root, binding, package, &version, &names)?);
    }
    let mut out = front_matter(
        "Adapters",
        "Adapters",
        "Every model binding of the inference adapters: what it claims, the features it needs and what it measured.",
        2,
    );
    out.push_str(&format!(
        "# Adapters\n\nGenerated by `inference-docs` from the records the adapters commit. A \
         hosted binding's claims are a claims record that a test in its adapter holds equal to the \
         code; a local binding's claims are the capabilities its capability record gives evidence \
         for, each evidence a named test of the adapter. Every adapter is off until you depend on \
         it and turn on the features below. [Contracts](../concepts/contracts.md) says what each \
         capability means.\n\n## Bindings\n\n\
         Status names the release whose [changelog]({SOURCE}/CHANGELOG.md) section introduced \
         the binding: \"Since 0.x.0\" is in that release and every one after it.\n\n\
         | Binding | Package | Runs | Model | Features | Status |\n\
         |---|---|---|---|---|---|\n",
    ));
    for read in &reads {
        out.push_str(&format!(
            "| {} | [`{}`]({TREE}/{}) | {} | `{}` | {} | Since {} |\n",
            read.binding.name,
            read.binding.package,
            all.iter()
                .find(|package| package.name == read.binding.package)
                .map(|package| package.directory.as_str())
                .unwrap_or_default(),
            cell(read.binding.runs),
            cell(&read.model),
            code_list(&read.features),
            read.binding.since,
        ));
    }
    out.push_str(
        "\n## Environment, licences and records\n\nWhat each binding reads from the environment, \
         the licences of its code and weights or where its data goes, and the record its claims \
         come from.\n\n",
    );
    for read in &reads {
        out.push_str(&format!(
            "- **{}**. Environment: {}. Licences or data: {}. Record: [{}]({SOURCE}/{}).\n",
            read.binding.name,
            cell(read.binding.environment),
            cell(read.binding.licence),
            read.path.rsplit('/').next().unwrap_or_default(),
            read.path,
        ));
    }
    out.push_str("\n## Claims\n\nEvery capability of the contract and the bindings that claim it, separated by semicolons. A capability nobody claims is still part of the contract: a pipeline that requires it is refused at planning, naming the node.\n\n| Capability | Claimed by |\n|---|---|\n");
    for name in &names {
        let claimed: Vec<_> = reads
            .iter()
            .filter(|read| read.claims.contains(name))
            .map(|read| read.binding.name)
            .collect();
        let claimed = if claimed.is_empty() {
            "none".to_owned()
        } else {
            claimed.join("; ")
        };
        out.push_str(&format!("| `{name}` | {claimed} |\n"));
    }
    out.push_str(
        "\n## Measured on the reference machine\n\nFrom each local binding's capability record. \
         The CPU real-time factor is compute time over audio time; below 1 keeps up.\n\n\
         | Binding | Peak resident memory | Load time | CPU real-time factor | GPU memory |\n\
         |---|---|---|---|---|\n",
    );
    let mut hardware = Vec::new();
    for read in &reads {
        let Some((machine, profile)) = &read.measured else {
            continue;
        };
        if !hardware.contains(machine) {
            hardware.push(machine.clone());
        }
        out.push_str(&format!(
            "| {} | {} MiB | {} ms | {} | {} |\n",
            read.binding.name,
            grouped(profile.peak_resident_memory_mib),
            grouped(profile.load_time_ms),
            profile
                .cpu_real_time_factor_milli
                .map_or("n/a".to_owned(), |milli| format!(
                    "{}.{:03}",
                    milli / 1000,
                    milli % 1000
                )),
            profile
                .gpu_memory_mib
                .map_or("none".to_owned(), |mib| format!("{} MiB", grouped(mib))),
        ));
    }
    out.push_str("\nThe machines, as the records name them:\n\n");
    for machine in hardware {
        out.push_str(&format!("- {}\n", cell(&machine)));
    }
    Ok(out)
}

/// The adapter reference for the repository at `root`.
pub fn page(root: &Path, metadata: &Value) -> Result<String> {
    page_for(root, metadata, BINDINGS)
}

/// Each binding's name, the release it shipped in, and whether it is hosted.
#[cfg(test)]
pub fn releases() -> impl Iterator<Item = (&'static str, &'static str, bool)> {
    BINDINGS.iter().map(|binding| {
        (
            binding.name,
            binding.since,
            matches!(binding.record, Record::Claims(_)),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crates;

    fn metadata() -> Value {
        crates::metadata(&crate::repository_root()).unwrap()
    }

    #[test]
    fn the_page_lists_every_claim_of_every_record() {
        let page = page(&crate::repository_root(), &metadata()).unwrap();
        assert!(
            page.contains("| Capability | Claimed by |\n|---|---|\n")
                && page.contains("| `native_duplex` | OpenAI GPT-Live |\n")
                && page.contains("| `revisable_input` | none |\n"),
            "{page}"
        );
        assert!(page.contains("| sherpa-onnx streaming STT | 363 MiB | 4,937 ms | 0.131 | none |"));
    }

    #[test]
    fn the_bindings_table_keeps_the_long_cells_out_in_a_list() {
        let page = page(&crate::repository_root(), &metadata()).unwrap();
        assert!(
            page.contains(
                "| Binding | Package | Runs | Model | Features | Status |\n|---|---|---|---|---|---|\n"
            ),
            "{page}"
        );
        let list = page
            .split_once("\n## Environment, licences and records\n")
            .unwrap()
            .1;
        assert!(
            list.contains("- **whisper.cpp batch STT**. Environment: `INFERENCE_WHISPERCPP_STT_MODEL`. Licences or data: code MIT (whisper.cpp), Unlicense (whisper-rs); weights MIT. Record: [stt-i9-10900k-cpu.json]("),
            "{list}"
        );
    }

    #[test]
    fn a_record_of_another_revision_or_naming_a_missing_test_is_refused() {
        let root = crate::scratch("adapters");
        let metadata = metadata();
        let openai = packages(&metadata)
            .unwrap()
            .into_iter()
            .find(|package| package.name == "inference-openai")
            .unwrap();
        let version = workspace_version(&metadata).unwrap();
        let dir = root.join(&openai.directory);
        fs::create_dir_all(dir.join("records")).unwrap();
        fs::create_dir_all(dir.join("tests")).unwrap();
        fs::write(dir.join("tests/claims.rs"), "#[test]\nfn holds() {}\n").unwrap();
        let binding = Binding {
            name: "B",
            package: "inference-openai",
            runs: "r",
            features: &[],
            record: Record::Claims("records/c.json"),
            environment: "e",
            licence: "l",
            since: "0.1.0",
        };
        let write = |revision: &str, held_by: &str, claim: &str| {
            fs::write(
                dir.join("records/c.json"),
                serde_json::json!({
                    "model": "m",
                    "binding": "inference_openai::x",
                    "features": ["duplex"],
                    "claims": [claim],
                    "held_by": held_by,
                    "adapter_revision": revision,
                })
                .to_string(),
            )
            .unwrap();
        };
        let names = capability_names().unwrap();
        write(&version, "claims::holds", "output_streaming");
        assert!(read(&root, &binding, &openai, &version, &names).is_ok());
        write("0.0.1", "claims::holds", "output_streaming");
        let error = read(&root, &binding, &openai, &version, &names)
            .err()
            .unwrap();
        assert!(error.contains("adapter revision 0.0.1"), "{error}");
        write(&version, "claims::gone", "output_streaming");
        let error = read(&root, &binding, &openai, &version, &names)
            .err()
            .unwrap();
        assert!(error.contains("has no test gone"), "{error}");
        write(&version, "claims::holds", "telepathy");
        let error = read(&root, &binding, &openai, &version, &names)
            .err()
            .unwrap();
        assert!(
            error.contains("telepathy, which is not a capability"),
            "{error}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// Per binding, a phrase of the `CHANGELOG.md` entry that introduced it.
    const INTRODUCED: &[(&str, &str)] = &[
        (
            "OpenAI Realtime",
            "OpenAI Realtime as an `inference-duplex` native speech model",
        ),
        (
            "OpenAI GPT-Live",
            "OpenAI GPT-Live (`v1/live/sessions`) behind feature `gpt-live`",
        ),
        (
            "Amazon Nova Sonic on Bedrock",
            "Amazon Nova Sonic as an `inference-duplex` native speech model",
        ),
        (
            "ElevenLabs streaming TTS",
            "`inference-elevenlabs`, the first hosted TTS adapter",
        ),
        (
            "OpenAI streamed speech TTS",
            "`inference-openai`: feature `speech`, `speech::OpenAiSpeech`",
        ),
        (
            "ElevenLabs agent conversation",
            "`ElevenLabsAgent` in `inference-elevenlabs`, behind feature `duplex`",
        ),
        (
            "Cartesia Sonic streaming TTS",
            "`inference-cartesia`: a new adapter crate",
        ),
        (
            "Deepgram streaming STT",
            "`DeepgramRecogniser`, Deepgram streaming speech-to-text",
        ),
        (
            "ElevenLabs realtime STT",
            "`ElevenLabsRecogniser`, ElevenLabs realtime speech-to-text",
        ),
        (
            "Deepgram Voice Agent",
            "`DeepgramAgent` is a Deepgram Voice Agent session",
        ),
        (
            "Phonic speech-to-speech",
            "`inference-phonic`: a new adapter crate",
        ),
        (
            "Deepslate Opal",
            "`inference-deepslate`: a new adapter crate",
        ),
        (
            "sherpa-onnx streaming STT",
            "the sherpa-onnx streaming recogniser as a local STT adapter",
        ),
        (
            "whisper.cpp batch STT",
            "whisper.cpp as a local batch STT adapter",
        ),
        (
            "llama-server LLM",
            "a local llama.cpp `llama-server` as an output-streaming LLM",
        ),
        (
            "llama-server LLM, tools on",
            "tool proposals as structured events, and one capability record per configuration",
        ),
    ];

    /// One released section of `CHANGELOG.md`, its text and its `### Added` list with their
    /// whitespace collapsed, so a phrase matches across the file's line breaks.
    struct Section {
        version: String,
        text: String,
        added: String,
    }

    fn collapsed(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// The released sections of `changelog`, oldest first.
    fn released(changelog: &str) -> Vec<Section> {
        let mut sections: Vec<_> = changelog
            .split("\n## [")
            .skip(1)
            .filter_map(|section| {
                let (version, text) = section.split_once(']')?;
                (version != "Unreleased").then(|| Section {
                    version: version.to_owned(),
                    text: collapsed(text),
                    added: text
                        .split("\n### ")
                        .find(|list| list.starts_with("Added\n"))
                        .map(collapsed)
                        .unwrap_or_default(),
                })
            })
            .collect();
        sections.reverse();
        sections
    }

    #[test]
    fn every_binding_is_since_the_release_whose_changelog_section_introduced_it() {
        let changelog = fs::read_to_string(crate::repository_root().join("CHANGELOG.md")).unwrap();
        let sections = released(&changelog);
        assert!(sections.len() > 20, "{} released sections", sections.len());
        let mut wrong = Vec::new();
        for binding in BINDINGS {
            let Some((_, phrase)) = INTRODUCED.iter().find(|(name, _)| *name == binding.name)
            else {
                wrong.push(format!("{}: no introducing phrase", binding.name));
                continue;
            };
            match sections
                .iter()
                .find(|section| section.text.contains(phrase))
            {
                Some(section)
                    if section.version == binding.since && section.added.contains(phrase) => {}
                Some(section) => wrong.push(format!(
                    "{}: since {:?}, but {phrase:?} is first in the {} section{}",
                    binding.name,
                    binding.since,
                    section.version,
                    if section.added.contains(phrase) {
                        ""
                    } else {
                        ", outside its Added list"
                    }
                )),
                None => wrong.push(format!(
                    "{}: since {:?}, but no released section says {phrase:?}",
                    binding.name, binding.since
                )),
            }
        }
        for (name, _) in INTRODUCED {
            if !BINDINGS.iter().any(|binding| binding.name == *name) {
                wrong.push(format!("{name}: no such binding"));
            }
        }
        assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    }

    #[test]
    fn every_row_of_the_bindings_table_names_the_release_it_shipped_in() {
        let page = page(&crate::repository_root(), &metadata()).unwrap();
        let table = page
            .split_once("\n## Bindings\n")
            .unwrap()
            .1
            .split_once("\n## ")
            .unwrap()
            .0;
        assert!(
            table.contains(
                "| Binding | Package | Runs | Model | Features | Status |\n|---|---|---|---|---|---|\n"
            ),
            "{table}"
        );
        let rows: Vec<_> = table
            .lines()
            .filter(|line| line.starts_with("| ") && !line.starts_with("| Binding |"))
            .collect();
        assert_eq!(rows.len(), BINDINGS.len(), "{table}");
        for (row, binding) in rows.iter().zip(BINDINGS) {
            assert!(row.starts_with(&format!("| {} |", binding.name)), "{row}");
            assert!(
                row.ends_with(&format!(" | Since {} |", binding.since)),
                "{row}"
            );
        }
        assert!(
            rows.iter()
                .any(|row| row.starts_with("| ElevenLabs realtime STT |")
                    && row.ends_with(" | Since 0.28.0 |")),
            "{table}"
        );
    }

    #[test]
    fn numbers_are_grouped_by_thousands() {
        assert_eq!(grouped(7), "7");
        assert_eq!(grouped(4937), "4,937");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }
}
