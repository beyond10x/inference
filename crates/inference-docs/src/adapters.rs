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
    },
    Binding {
        name: "OpenAI GPT-Live",
        package: "inference-openai",
        runs: "native speech (duplex); hosted, WebSocket (`v1/live/sessions`)",
        features: &[],
        record: Record::Claims("records/claims-gpt-live.json"),
        environment: "`OPENAI_API_KEY`, or the keychain secret `openai-api-key` with feature `keychain`",
        licence: "hosted service: audio leaves the machine; billed per session second",
    },
    Binding {
        name: "Amazon Nova Sonic on Bedrock",
        package: "inference-aws",
        runs: "native speech (duplex); hosted, bidirectional stream",
        features: &[],
        record: Record::Claims("records/claims-nova-sonic.json"),
        environment: "the AWS default credential chain; `AWS_REGION`; `INFERENCE_NOVA_SONIC_MODEL`",
        licence: "hosted service: audio leaves the machine",
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
    },
    Binding {
        name: "whisper.cpp batch STT",
        package: "inference-whispercpp",
        runs: "STT, batch; in process, native library, CPU",
        features: &[],
        record: Record::Capability("records/stt-i9-10900k-cpu.json", "Whisper base.en"),
        environment: "`INFERENCE_WHISPERCPP_STT_MODEL`",
        licence: "code MIT (whisper.cpp), Unlicense (whisper-rs); weights MIT",
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
    out.push_str(
        "# Adapters\n\nGenerated by `inference-docs` from the records the adapters commit. A \
         hosted binding's claims are a claims record that a test in its adapter holds equal to the \
         code; a local binding's claims are the capabilities its capability record gives evidence \
         for, each evidence a named test of the adapter. Every adapter is off until you depend on \
         it and turn on the features below. [Contracts](../concepts/contracts.md) says what each \
         capability means.\n\n## Bindings\n\n\
         | Binding | Package | Runs | Model | Features |\n\
         |---|---|---|---|---|\n",
    );
    for read in &reads {
        out.push_str(&format!(
            "| {} | [`{}`]({TREE}/{}) | {} | `{}` | {} |\n",
            read.binding.name,
            read.binding.package,
            all.iter()
                .find(|package| package.name == read.binding.package)
                .map(|package| package.directory.as_str())
                .unwrap_or_default(),
            cell(read.binding.runs),
            cell(&read.model),
            code_list(&read.features),
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
                "| Binding | Package | Runs | Model | Features |\n|---|---|---|---|---|\n"
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

    #[test]
    fn numbers_are_grouped_by_thousands() {
        assert_eq!(grouped(7), "7");
        assert_eq!(grouped(4937), "4,937");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }
}
