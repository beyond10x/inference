//! The constraints of the owner's exception for espeak-ng, each held here: espeak-ng is a
//! code-licence entry of its own beside sherpa-onnx's, in the model the adapter declares and in
//! the crate's README; nothing of espeak-ng or of the native library is in the repository; and
//! the adapter is opt-in, a dependency of no contract crate and of no other adapter, as Cargo
//! reads the manifests.
//!
//! The code-licence list is held to the native archive: every library the build script of the
//! pinned `sherpa-onnx-sys` names is covered by one entry, every entry has an SPDX expression
//! and a source pinned to a version, tag or commit, and the entries are the 29 libraries whose
//! licence was read for the archive built with text-to-speech. What the adapter declares about
//! itself is held here too. Needs no weights and no network.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use inference_sherpa_onnx_tts::inference_tts::CONTRACT_REVISION;
use inference_sherpa_onnx_tts::inference_tts::inference_core::model::{
    Capability, ExecutionForm, Locality, Modality,
};
use inference_sherpa_onnx_tts::{
    ARCHIVE_SHA256, BACKEND, CHECKPOINT, CODE_LICENSES, CodeLicense, DEFAULT_MAX_CONCURRENCY,
    ESPEAK_NG_DATA, FILES, NATIVE_VERSION, SOURCE_REVISION, VOICES, WEIGHT_LICENSE, checksum,
    code_license, declare_adapter, declare_model, native_version,
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

/// The names of the library list in the build script of the `sherpa-onnx-sys` that `Cargo.lock`
/// pins. The script is read from the source Cargo unpacked on this machine to build the crate,
/// below the registry of Cargo's home: no network, and a pin that moves is read at its new
/// version.
fn build_script_libraries() -> Vec<String> {
    let lock = read(&repository().join("Cargo.lock"));
    let mut versions = Vec::new();
    let mut lines = lock.lines();
    while let Some(line) = lines.next() {
        if line == "name = \"sherpa-onnx-sys\"" {
            let version = lines
                .next()
                .and_then(|line| line.strip_prefix("version = \""))
                .and_then(|rest| rest.strip_suffix('"'))
                .expect("Cargo.lock gives a package's version below its name");
            versions.push(version);
        }
    }
    let [version] = versions.as_slice() else {
        panic!("Cargo.lock pins one sherpa-onnx-sys: {versions:?}");
    };
    let home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cargo")))
        .expect("CARGO_HOME or HOME says where Cargo keeps its registry");
    let sources = home.join("registry").join("src");
    let mut scripts: Vec<PathBuf> = fs::read_dir(&sources)
        .unwrap_or_else(|error| panic!("{}: {error}", sources.display()))
        .map(|entry| {
            entry
                .expect("the entry is read")
                .path()
                .join(format!("sherpa-onnx-sys-{version}"))
                .join("build.rs")
        })
        .filter(|script| script.is_file())
        .collect();
    scripts.sort();
    let script = scripts.first().unwrap_or_else(|| {
        panic!(
            "no build.rs of sherpa-onnx-sys {version} below {}: the case reads the source \
             Cargo built this crate from",
            sources.display()
        )
    });
    let text = read(script);
    let list = text
        .split_once("const SHERPA_ONNX_STATIC_LIBS: &[&str] = &[")
        .and_then(|(_, rest)| rest.split_once("];"))
        .map(|(list, _)| list)
        .unwrap_or_else(|| panic!("{} has no library list", script.display()));
    let names: Vec<String> = list
        .lines()
        .map(|line| line.trim().trim_end_matches(','))
        .filter(|line| !line.is_empty())
        .map(|line| {
            line.strip_prefix('"')
                .and_then(|line| line.strip_suffix('"'))
                .unwrap_or_else(|| panic!("{}: `{line}` is not a name", script.display()))
                .to_owned()
        })
        .collect();
    assert!(
        names.iter().any(|name| name == "sherpa-onnx-c-api"),
        "{} lists the libraries: {names:?}",
        script.display()
    );
    names
}

/// The licence identifiers of the reading, as SPDX writes them, and the one exception.
const IDENTIFIERS: [&str; 11] = [
    "Apache-2.0",
    "MIT",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "BSL-1.0",
    "Zlib",
    "MPL-2.0",
    "Unlicense",
    "Unicode-DFS-2016",
    "GPL-3.0-or-later",
    "LGPL-2.1-or-later",
];
const EXCEPTION: &str = "GCC-exception-3.1";

/// Whether `license` is an SPDX expression over [`IDENTIFIERS`]: identifiers joined by `AND` or
/// `OR`, an identifier at most once `WITH` the exception.
fn is_an_expression_of_the_identifiers_read(license: &str) -> bool {
    let mut tokens = license.split(' ');
    loop {
        if !tokens
            .next()
            .is_some_and(|identifier| IDENTIFIERS.contains(&identifier))
        {
            return false;
        }
        let mut next = tokens.next();
        if next == Some("WITH") {
            if tokens.next() != Some(EXCEPTION) {
                return false;
            }
            next = tokens.next();
        }
        match next {
            None => return true,
            Some("AND" | "OR") => {}
            Some(_) => return false,
        }
    }
}

/// Whether the entry's source is a URL of one file at the entry's revision: `…/<revision>/<file>`,
/// or the form of a git web view, `…;f=<file>;hb=…/<revision>`. A branch is no revision.
fn is_pinned(entry: &CodeLicense) -> bool {
    let revision = entry.revision;
    let a_file_below_the_revision = entry
        .source
        .split_once(&format!("/{revision}/"))
        .is_some_and(|(_, file)| !file.is_empty());
    let a_file_of_a_web_view =
        entry.source.contains(";f=") && entry.source.ends_with(&format!("/{revision}"));
    entry.source.starts_with("https://")
        && !revision.is_empty()
        && !["main", "master", "HEAD", "trunk", "develop"].contains(&revision)
        && (a_file_below_the_revision || a_file_of_a_web_view)
}

/// The libraries the native archive built with text-to-speech links and the licence read for
/// each, on 2026-11-25: this case's own copy of the reading, so that an entry added to, dropped
/// from or changed in [`CODE_LICENSES`] alone is red.
const READ: [(&str, &str); 29] = [
    ("sherpa-onnx", "Apache-2.0"),
    ("kaldi-native-fbank", "Apache-2.0"),
    ("kissfft", "BSD-3-Clause"),
    ("kaldi-decoder", "Apache-2.0"),
    ("kaldifst", "Apache-2.0"),
    ("openfst", "Apache-2.0"),
    ("Eigen", "MPL-2.0"),
    (
        "simple-sentencepiece",
        "Apache-2.0 AND BSD-2-Clause AND Zlib",
    ),
    ("nlohmann/json", "MIT"),
    ("hclust-cpp", "BSD-2-Clause"),
    ("espeak-ng", "GPL-3.0-or-later"),
    ("ucd-tools", "GPL-3.0-or-later AND Unicode-DFS-2016"),
    ("piper-phonemize", "MIT"),
    ("uni-algo", "Unlicense OR MIT"),
    ("onnxruntime", "MIT"),
    ("abseil-cpp", "Apache-2.0"),
    ("onnx", "Apache-2.0"),
    ("protobuf", "BSD-3-Clause"),
    ("re2", "BSD-3-Clause"),
    ("flatbuffers", "Apache-2.0"),
    ("Eigen in onnxruntime", "MPL-2.0"),
    ("GSL", "MIT"),
    ("nlohmann/json in onnxruntime", "MIT"),
    ("date", "MIT"),
    ("Boost.Mp11", "BSL-1.0"),
    ("SafeInt", "MIT"),
    ("cpuinfo", "BSD-2-Clause"),
    (
        "libstdc++ and libgcc",
        "GPL-3.0-or-later WITH GCC-exception-3.1",
    ),
    ("glibc", "LGPL-2.1-or-later"),
];

#[test]
fn every_library_the_pinned_build_script_names_is_covered_by_one_code_licence_entry() {
    let named = build_script_libraries();
    let not_by_one: Vec<(&str, Vec<&str>)> = named
        .iter()
        .map(|name| {
            let covering: Vec<&str> = CODE_LICENSES
                .iter()
                .filter(|entry| entry.libraries.contains(&name.as_str()))
                .map(|entry| entry.component)
                .collect();
            (name.as_str(), covering)
        })
        .filter(|(_, covering)| covering.len() != 1)
        .collect();
    assert!(
        not_by_one.is_empty(),
        "each of the {} libraries the build script of the pinned sherpa-onnx-sys names is \
         covered by one entry of CODE_LICENSES; these are not, each with the entries that name \
         it: {not_by_one:?}",
        named.len()
    );
    let not_named: Vec<(&str, &str)> = CODE_LICENSES
        .iter()
        .flat_map(|entry| {
            entry
                .libraries
                .iter()
                .map(move |library| (entry.component, *library))
        })
        .filter(|(_, library)| !named.iter().any(|name| name == library))
        .collect();
    assert!(
        not_named.is_empty(),
        "an entry covers only libraries the build script names {named:?}; not so: {not_named:?}"
    );
}

#[test]
fn no_two_code_licence_entries_name_one_component_and_each_has_a_licence_and_a_pinned_source() {
    let mut components: Vec<&str> = CODE_LICENSES.iter().map(|entry| entry.component).collect();
    components.sort_unstable();
    let twice: Vec<&str> = components
        .windows(2)
        .filter(|pair| pair[0] == pair[1])
        .map(|pair| pair[0])
        .collect();
    assert!(
        twice.is_empty(),
        "components named by two entries: {twice:?}"
    );

    let not_joinable: Vec<&str> = components
        .iter()
        .copied()
        .filter(|component| component.is_empty() || component.contains([',', '(', ')']))
        .collect();
    assert!(
        not_joinable.is_empty(),
        "a component is named, without the comma and the parentheses that code_license joins \
         with: {not_joinable:?}"
    );

    let no_expression: Vec<(&str, &str)> = CODE_LICENSES
        .iter()
        .filter(|entry| !is_an_expression_of_the_identifiers_read(entry.license))
        .map(|entry| (entry.component, entry.license))
        .collect();
    assert!(
        no_expression.is_empty(),
        "every licence is an SPDX expression over {IDENTIFIERS:?} and {EXCEPTION}; not so: \
         {no_expression:?}"
    );

    let not_pinned: Vec<(&str, &str, &str)> = CODE_LICENSES
        .iter()
        .filter(|entry| !is_pinned(entry))
        .map(|entry| (entry.component, entry.revision, entry.source))
        .collect();
    assert!(
        not_pinned.is_empty(),
        "every source is the URL of a file at the entry's version, tag or commit, no branch \
         and no bare repository; not so (component, revision, source): {not_pinned:?}"
    );
}

#[test]
fn the_code_licence_list_is_the_29_libraries_read_for_the_archive_with_text_to_speech() {
    let listed: Vec<(&str, &str)> = CODE_LICENSES
        .iter()
        .map(|entry| (entry.component, entry.license))
        .collect();
    assert_eq!(
        listed, READ,
        "CODE_LICENSES names the libraries of the reading, each with the licence read"
    );
}

/// The identifiers of the reading the repository's licence rule for local adapters admits for
/// backend code (`AGENTS.md`): Apache-2.0, MIT or more permissive. The list of
/// `inference-sherpa-onnx` lacks Unlicense and Unicode-DFS-2016, which no entry of its archive
/// carries, and stays that strict.
const INSIDE_THE_RULE: [&str; 8] = [
    "Apache-2.0",
    "MIT",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "BSL-1.0",
    "Zlib",
    "Unlicense",
    "Unicode-DFS-2016",
];

/// The entries outside that rule the owner admitted, each only as built into the upstream
/// archives the build fetches (`AGENTS.md`): espeak-ng by its named exception, the rest by the
/// owner's decision of 2026-11-27, ucd-tools as part of the espeak-ng exception and the
/// toolchain pieces under their runtime and linking exceptions. Each with the name the licence
/// bullet of `AGENTS.md` gives it.
const ADMITTED_OUTSIDE_THE_RULE: [(&str, &str, &str); 6] = [
    ("espeak-ng", "GPL-3.0-or-later", "espeak-ng"),
    ("Eigen", "MPL-2.0", "Eigen"),
    (
        "ucd-tools",
        "GPL-3.0-or-later AND Unicode-DFS-2016",
        "ucd-tools",
    ),
    ("Eigen in onnxruntime", "MPL-2.0", "Eigen"),
    (
        "libstdc++ and libgcc",
        "GPL-3.0-or-later WITH GCC-exception-3.1",
        "GCC runtime",
    ),
    ("glibc", "LGPL-2.1-or-later", "glibc"),
];

/// The bullet of `AGENTS.md` that states the licence rule for local adapters, its lines joined
/// by one space.
fn licence_bullet() -> String {
    let agents = read(&repository().join("AGENTS.md"));
    let mut lines = agents
        .lines()
        .skip_while(|line| !line.starts_with("- A local adapter's backend code"));
    let first = lines.next().expect("AGENTS.md states the licence rule");
    std::iter::once(first)
        .chain(lines.take_while(|line| line.starts_with("  ")))
        .map(str::trim)
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn every_entry_admitted_outside_the_licence_rule_is_named_by_the_rule_in_agents_md() {
    let bullet = licence_bullet();
    let not_named: Vec<&str> = ADMITTED_OUTSIDE_THE_RULE
        .iter()
        .map(|(_, _, named)| *named)
        .filter(|named| !bullet.contains(named))
        .collect();
    assert!(
        not_named.is_empty(),
        "the licence bullet of AGENTS.md names every entry admitted outside the rule; not \
         {not_named:?}: {bullet}"
    );
}

#[test]
fn every_code_licence_entry_is_inside_the_licence_rule_or_admitted_by_the_owner() {
    let outside: Vec<(&str, &str)> = CODE_LICENSES
        .iter()
        .filter(|entry| {
            !entry
                .license
                .split(' ')
                .filter(|token| !matches!(*token, "AND" | "OR"))
                .all(|identifier| INSIDE_THE_RULE.contains(&identifier))
        })
        .map(|entry| (entry.component, entry.license))
        .filter(|(component, license)| {
            !ADMITTED_OUTSIDE_THE_RULE
                .iter()
                .any(|(admitted, admitted_license, _)| {
                    admitted == component && admitted_license == license
                })
        })
        .collect();
    assert!(
        outside.is_empty(),
        "every entry is under {INSIDE_THE_RULE:?} alone or admitted by the owner as \
         {ADMITTED_OUTSIDE_THE_RULE:?}; not so: {outside:?}"
    );
}

#[test]
fn espeak_ng_is_a_code_licence_entry_of_its_own_under_the_gpl_version_3_or_later() {
    let entry_of = |component: &str| {
        let mut entries = CODE_LICENSES
            .iter()
            .filter(|entry| entry.component == component);
        match (entries.next(), entries.next()) {
            (Some(entry), None) => entry,
            _ => panic!("{component} has exactly one entry: {CODE_LICENSES:?}"),
        }
    };
    assert_eq!(entry_of("sherpa-onnx").license, "Apache-2.0");
    let espeak_ng = entry_of("espeak-ng");
    assert_eq!(espeak_ng.license, "GPL-3.0-or-later");
    assert_eq!(
        espeak_ng.libraries,
        ["espeak-ng"],
        "the entry is espeak-ng's alone: it covers no other library of the build script"
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
                && line.contains(entry.source)
                && entry
                    .libraries
                    .iter()
                    .all(|library| line.contains(&format!("`{library}`")))),
            "the README has a row for {} under {} at {}, naming {:?}",
            entry.component,
            entry.license,
            entry.source,
            entry.libraries
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

/// The packages of the workspace as Cargo itself reads their manifests: `cargo metadata
/// --no-deps`, which resolves nothing and reaches no network. A dependency is seen there
/// whatever form its line has: a table, a renamed, an inherited, an unspaced or a quoted key.
fn workspace_packages() -> Vec<serde_json::Value> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--offline",
        ])
        .current_dir(repository())
        .output()
        .expect("cargo is run");
    assert!(
        output.status.success(),
        "cargo metadata: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("cargo metadata prints JSON");
    metadata["packages"]
        .as_array()
        .expect("the metadata lists the packages")
        .clone()
}

#[test]
fn the_adapter_is_opt_in_a_dependency_of_the_offline_cascade_example_alone() {
    let repository = repository();
    let packages = workspace_packages();

    let read_by_cargo: Vec<PathBuf> = packages
        .iter()
        .filter_map(|package| package["manifest_path"].as_str())
        .map(PathBuf::from)
        .collect();
    let unread: Vec<PathBuf> = tree(&repository)
        .into_iter()
        .filter(|path| path.file_name().is_some_and(|name| name == "Cargo.toml"))
        .filter(|path| *path != repository.join("Cargo.toml") && !read_by_cargo.contains(path))
        .filter(|path| read(path).contains(CRATE))
        .collect();
    assert!(
        unread.is_empty(),
        "every manifest that names {CRATE} is one of a package of the workspace, so Cargo's \
         reading covers it: {unread:?}"
    );

    let mut dependents = Vec::new();
    for package in &packages {
        let name = package["name"].as_str().expect("a package has a name");
        let dependencies = package["dependencies"]
            .as_array()
            .expect("a package lists its dependencies");
        for dependency in dependencies {
            if dependency["name"].as_str() == Some(CRATE) {
                dependents.push((name.to_owned(), dependency.clone()));
            }
        }
    }
    let [(dependent, dependency)] = dependents.as_slice() else {
        panic!(
            "one package depends on {CRATE}, the offline cascade example, and no contract crate, \
             no other adapter and not the inference crate: {dependents:?}"
        );
    };
    assert_eq!(dependent, "local-voice-cascade");
    assert_eq!(
        (
            dependency["optional"].as_bool(),
            dependency["kind"].as_str()
        ),
        (Some(true), None),
        "the example takes {CRATE} as an optional dependency of its library: {dependency}"
    );

    let cascade = packages
        .iter()
        .find(|package| package["name"].as_str() == Some("local-voice-cascade"))
        .expect("the example is a package of the workspace");
    let features = cascade["features"]
        .as_object()
        .expect("a package lists its features");
    let turned_on_by: Vec<&str> = features
        .iter()
        .filter(|(_, parts)| {
            parts.as_array().is_some_and(|parts| {
                parts
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .any(|part| {
                        let part = part.strip_prefix("dep:").unwrap_or(part);
                        part.split(['/', '?']).next() == Some(CRATE)
                    })
            })
        })
        .map(|(feature, _)| feature.as_str())
        .collect();
    assert_eq!(
        turned_on_by,
        ["local"],
        "only the example's feature `local` turns {CRATE} on"
    );
    let on_by_default = |feature: &str| {
        let mut reached = vec!["default".to_owned()];
        let mut at = 0;
        while let Some(current) = reached.get(at).cloned() {
            at += 1;
            let parts = features.get(&current).and_then(serde_json::Value::as_array);
            for part in parts.into_iter().flatten().filter_map(|part| part.as_str()) {
                if !reached.iter().any(|reached| reached == part) {
                    reached.push(part.to_owned());
                }
            }
        }
        reached.iter().any(|reached| reached == feature)
    };
    assert!(
        !on_by_default("local"),
        "the example's default features do not turn `local` on: {features:?}"
    );
}
