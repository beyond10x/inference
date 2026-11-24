//! The constraints of the owner's exception for espeak-ng, each held here: espeak-ng is a
//! code-licence entry of its own beside sherpa-onnx's, in the model the adapter declares and in
//! the crate's README; nothing of espeak-ng or of the native library is in the repository; and
//! the adapter is opt-in, a dependency of no contract crate and of no other adapter. What the
//! adapter declares about itself is held here too. Needs no weights.

use std::fs;
use std::path::{Path, PathBuf};

use inference_sherpa_onnx_tts::inference_tts::CONTRACT_REVISION;
use inference_sherpa_onnx_tts::inference_tts::inference_core::model::{
    Capability, ExecutionForm, Locality, Modality,
};
use inference_sherpa_onnx_tts::{
    ARCHIVE_SHA256, BACKEND, CHECKPOINT, CODE_LICENSES, DEFAULT_MAX_CONCURRENCY, ESPEAK_NG_DATA,
    FILES, NATIVE_VERSION, SOURCE_REVISION, VOICES, WEIGHT_LICENSE, checksum, code_license,
    declare_adapter, declare_model, native_version,
};

const CRATE: &str = "inference-sherpa-onnx-tts";

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the crate is two directories below the repository")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// Every file and directory below `dir`, but build output, installed packages and hidden
/// directories, which are not part of the repository's tree.
fn tree(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut directories = vec![dir.to_path_buf()];
    while let Some(directory) = directories.pop() {
        let entries = fs::read_dir(&directory)
            .unwrap_or_else(|error| panic!("{}: {error}", directory.display()));
        for entry in entries {
            let path = entry.expect("the entry is read").path();
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            if name.starts_with('.') || name == "target" || name == "node_modules" {
                continue;
            }
            if path.is_dir() {
                directories.push(path.clone());
            }
            found.push(path);
        }
    }
    found
}

#[test]
fn espeak_ng_is_a_code_licence_entry_of_its_own_beside_sherpa_onnx() {
    let license_of = |component: &str| {
        let mut entries = CODE_LICENSES
            .iter()
            .filter(|entry| entry.component.starts_with(component));
        match (entries.next(), entries.next()) {
            (Some(entry), None) => entry.license,
            _ => panic!("{component} has exactly one entry: {CODE_LICENSES:?}"),
        }
    };
    assert_eq!(license_of("sherpa-onnx"), "Apache-2.0");
    assert_eq!(license_of("espeak-ng"), "GPL-3.0");
    assert_eq!(
        CODE_LICENSES
            .iter()
            .filter(|entry| entry.license.contains("GPL"))
            .map(|entry| entry.component)
            .collect::<Vec<_>>(),
        ["espeak-ng"],
        "espeak-ng is the one component under the GPL"
    );
}

#[test]
fn the_declared_model_carries_every_code_licence_entry_and_the_pins() {
    let (model, declared) = declare_model();
    assert_eq!(declared.checkpoint, CHECKPOINT);
    assert_eq!(model.code_license(), code_license());
    for entry in CODE_LICENSES {
        assert!(
            model
                .code_license()
                .contains(&format!("{} ({})", entry.license, entry.component)),
            "{}: {}",
            entry.component,
            model.code_license()
        );
    }
    assert_eq!(model.weight_license(), WEIGHT_LICENSE);
    assert_eq!(model.weight_license(), "Apache-2.0");
    assert_eq!(model.source_revision(), SOURCE_REVISION);
    assert_eq!(model.checksum(), checksum());
    for file in FILES {
        assert!(
            model
                .checksum()
                .contains(&format!("sha256:{} {}", file.sha256, file.name)),
            "{}",
            model.checksum()
        );
    }
    assert!(
        model.checksum().contains(&format!(
            "sha256:{} {}/",
            ESPEAK_NG_DATA.sha256, ESPEAK_NG_DATA.name
        )),
        "{}",
        model.checksum()
    );
    assert!(
        model.provenance().contains(ARCHIVE_SHA256) && model.provenance().contains("espeak-ng"),
        "the provenance names the archive by its sha256 and the phoneme data it carries: {}",
        model.provenance()
    );
    assert_eq!(model.input_modalities(), [Modality::Text]);
    assert_eq!(model.output_modalities(), [Modality::Audio]);
}

#[test]
fn the_adapter_is_native_ffi_in_the_process_and_names_its_voices_and_codec() {
    let (model, _) = declare_model();
    let (adapter, declared) = declare_adapter(model.model_id(), DEFAULT_MAX_CONCURRENCY)
        .expect("the default maximum concurrency is positive");
    assert_eq!(declared.model_id, model.model_id());
    assert_eq!(adapter.backend(), format!("{BACKEND} ({CHECKPOINT})"));
    assert_eq!(adapter.contract_revision(), CONTRACT_REVISION);
    assert_eq!(adapter.execution_form(), ExecutionForm::NativeFfi);
    assert_eq!(adapter.locality(), Locality::SameProcess);
    assert_eq!(
        adapter.capabilities(),
        [Capability::IncrementalInput, Capability::OutputStreaming]
    );
    assert_eq!(adapter.max_concurrency(), 1);
    assert_eq!(adapter.descriptor().voices, VOICES);
    assert_eq!(adapter.descriptor().codecs, ["pcm_s16le/24000/1"]);
    assert!(declare_adapter(model.model_id(), 0).is_err());
}

#[test]
fn the_linked_native_library_is_the_version_the_backend_names() {
    assert_eq!(native_version(), NATIVE_VERSION);
    assert!(BACKEND.ends_with(NATIVE_VERSION), "{BACKEND}");
}

#[test]
fn the_readme_carries_every_code_licence_entry_and_the_exception() {
    let readme = read(&Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"));
    for entry in CODE_LICENSES {
        assert!(
            readme.lines().any(|line| line.contains(entry.component)
                && line.contains(entry.license)
                && line.contains(entry.source)),
            "the README has a row for {} under {} at {}",
            entry.component,
            entry.license,
            entry.source
        );
    }
    assert!(
        readme.contains("approval-record:sherpa-tts-gpl-phonemiser"),
        "the README cites the owner's exception"
    );
}

#[test]
fn nothing_of_espeak_ng_or_of_the_native_library_is_in_the_repository() {
    let forbidden = |path: &Path| {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let extension = path
            .extension()
            .map(|extension| extension.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        name.contains("espeak")
            || name.starts_with("libsherpa-onnx")
            || name.starts_with("libonnxruntime")
            || name.starts_with("libpiper")
            || ["so", "bz2", "onnx", "dylib", "dll"].contains(&extension.as_str())
            || FILES
                .iter()
                .any(|file| name == file.name && name != "tokens.txt")
    };
    let found: Vec<PathBuf> = tree(&repository())
        .into_iter()
        .filter(|path| forbidden(path))
        .collect();
    assert!(
        found.is_empty(),
        "the native library, its archives, model files and espeak-ng's data are fetched into \
         target/, never committed: {found:?}"
    );
}

#[test]
fn the_adapter_is_opt_in_a_dependency_of_the_offline_cascade_example_alone() {
    let repository = repository();
    let dependents: Vec<PathBuf> = tree(&repository)
        .into_iter()
        .filter(|path| path.file_name().is_some_and(|name| name == "Cargo.toml"))
        .filter(|path| {
            read(path)
                .lines()
                .any(|line| line.trim_start().starts_with(&format!("{CRATE} ")))
        })
        .map(|path| {
            path.strip_prefix(&repository)
                .expect("the manifest is in the repository")
                .to_path_buf()
        })
        .collect();
    assert!(
        dependents
            .iter()
            .all(|path| path == Path::new("examples/local-voice-cascade/Cargo.toml")),
        "no contract crate, no other adapter and not the inference crate depends on {CRATE}: \
         {dependents:?}"
    );
    let cascade = read(&repository.join("examples/local-voice-cascade/Cargo.toml"));
    if let Some(line) = cascade
        .lines()
        .find(|line| line.trim_start().starts_with(&format!("{CRATE} ")))
    {
        assert!(
            line.contains("optional = true"),
            "the example depends on {CRATE} behind its feature `local` only: {line}"
        );
    }
}
