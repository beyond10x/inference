//! The native code this crate links carries no GPL-licensed phonemiser: no espeak-ng and no
//! piper-phonemize symbol is in the test binary or in any sherpa-onnx or onnxruntime library it
//! loaded. sherpa-onnx's default build turns text-to-speech on and links both, so a version bump
//! or an archive swap that brings them back fails here. Needs no weights.
//!
//! The code-licence list is held to the native archive too: every library the build script of
//! the pinned `sherpa-onnx-sys` names is covered by one entry or is one of the three the archive
//! without text-to-speech does not carry, every entry has an SPDX expression and a source pinned
//! to a version, tag or commit, the entries are the 25 libraries whose licence was read for that
//! archive, and `CODE_LICENSE` is the list joined. Needs no network.
#![cfg(all(feature = "stt", target_os = "linux"))]

use std::fs;
use std::path::{Path, PathBuf};

use inference_sherpa_onnx::stt::{
    CODE_LICENSE, CODE_LICENSES, CodeLicense, NATIVE_VERSION, native_version,
};

/// The symbol prefixes of espeak-ng (`espeak_`, as in `espeak_ng_Initialize`) and of
/// piper-phonemize's C++ namespace (`_ZN5piper`), each stored reversed so that this test's own
/// binary does not carry them. No item of this file may carry them in its name either: symbol
/// names are in the binary, and a test once named after the prefix failed on itself.
const FORBIDDEN_REVERSED: [&str; 2] = ["_kaepse", "repip5NZ_"];

/// A string only the sherpa-onnx implementation carries, `OnlineRecognizerConfig(`, stored
/// reversed: finding it shows the scan covered the native code, however it was linked.
const IMPLEMENTATION_REVERSED: &str = "(gifnoCrezingoceRenilnO";

fn forward(reversed: &str) -> Vec<u8> {
    reversed.bytes().rev().collect()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    let Some((first, rest)) = needle.split_first() else {
        return true;
    };
    let mut at = 0;
    while let Some(offset) = haystack[at..].iter().position(|byte| byte == first) {
        let start = at + offset + 1;
        if haystack[start..].starts_with(rest) {
            return true;
        }
        at = start;
    }
    false
}

/// The test binary and every sherpa-onnx or onnxruntime library mapped into this process.
fn linked_files() -> Vec<PathBuf> {
    let mut files = vec![std::env::current_exe().expect("the test binary has a path")];
    let maps = fs::read_to_string("/proc/self/maps").expect("Linux lists the mapped files");
    for line in maps.lines() {
        let Some(path) = line.split_whitespace().nth(5) else {
            continue;
        };
        let path = PathBuf::from(path);
        let native = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.contains("sherpa-onnx") || name.contains("onnxruntime"));
        if native && !files.contains(&path) {
            files.push(path);
        }
    }
    files
}

#[test]
fn the_linked_native_library_is_the_pinned_version() {
    assert_eq!(native_version(), NATIVE_VERSION);
}

#[test]
fn no_gpl_phonemiser_is_linked() {
    assert_eq!(
        native_version(),
        NATIVE_VERSION,
        "the native library is loaded"
    );
    let files = linked_files();
    let mut implementation_scanned = false;
    for path in &files {
        let bytes = fs::read(path).expect("a linked file is readable");
        implementation_scanned |= contains(&bytes, &forward(IMPLEMENTATION_REVERSED));
        for reversed in FORBIDDEN_REVERSED {
            let needle = forward(reversed);
            assert!(
                !contains(&bytes, &needle),
                "{} carries {}: a GPL-licensed phonemiser is linked",
                path.display(),
                String::from_utf8_lossy(&needle)
            );
        }
    }
    assert!(
        implementation_scanned,
        "none of {files:?} carries the sherpa-onnx implementation, so the scan shows nothing"
    );
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The names of the library list in the build script of the `sherpa-onnx-sys` that `Cargo.lock`
/// pins. The script is read from the source Cargo unpacked on this machine to build the crate,
/// below the registry of Cargo's home: no network, and a pin that moves is read at its new
/// version.
fn build_script_libraries() -> Vec<String> {
    let lock = read(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock"));
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

/// The three libraries the build script names that the archive without text-to-speech does not
/// carry, each with the reason: the phonemiser, its Unicode tables and the library that calls
/// it are built in only with text-to-speech, and the objects of this archive hold no symbol of
/// theirs. They have no entry in [`CODE_LICENSES`].
const NOT_LINKED: [(&str, &str); 3] = [
    (
        "piper_phonemize",
        "piper-phonemize is built in under the text-to-speech option only; no symbol of its \
         namespace is in the archive without it",
    ),
    (
        "espeak-ng",
        "the phonemiser is built in under the text-to-speech option only; no symbol of its C \
         interface is in the archive without it",
    ),
    (
        "ucd",
        "the Unicode tables come with the phonemiser's source tree; no symbol of theirs is in \
         the archive without text-to-speech",
    ),
];

/// The licence identifiers of the reading, as SPDX writes them, and the one exception.
const IDENTIFIERS: [&str; 9] = [
    "Apache-2.0",
    "MIT",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "BSL-1.0",
    "Zlib",
    "MPL-2.0",
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

/// The libraries the native archive built without text-to-speech links and the licence read for
/// each, on 2026-11-25: this case's own copy of the reading, so that an entry added to, dropped
/// from or changed in [`CODE_LICENSES`] alone is red.
const READ: [(&str, &str); 25] = [
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
fn every_library_the_pinned_build_script_names_has_one_code_licence_entry_or_is_not_linked() {
    let named = build_script_libraries();
    let excluded = |name: &str| NOT_LINKED.iter().any(|(library, _)| *library == name);
    let wrong: Vec<(&str, Vec<&str>, bool)> = named
        .iter()
        .map(|name| {
            let covering: Vec<&str> = CODE_LICENSES
                .iter()
                .filter(|entry| entry.libraries.contains(&name.as_str()))
                .map(|entry| entry.component)
                .collect();
            (name.as_str(), covering, excluded(name))
        })
        .filter(|(_, covering, excluded)| {
            if *excluded {
                !covering.is_empty()
            } else {
                covering.len() != 1
            }
        })
        .collect();
    assert!(
        wrong.is_empty(),
        "each of the {} libraries the build script of the pinned sherpa-onnx-sys names is \
         covered by one entry of CODE_LICENSES, or by none when the archive without \
         text-to-speech does not carry it; not so (library, entries that name it, whether it \
         is named as not linked): {wrong:?}",
        named.len()
    );
    let gone: Vec<&str> = NOT_LINKED
        .iter()
        .map(|(library, _)| *library)
        .filter(|library| !named.iter().any(|name| name == library))
        .collect();
    assert!(
        gone.is_empty(),
        "a library named as not linked is one the build script names {named:?}; not so: {gone:?}"
    );
    for (library, reason) in NOT_LINKED {
        assert!(!reason.is_empty(), "{library} is not linked for a reason");
    }
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
        "a component is named, without the comma and the parentheses that CODE_LICENSE joins \
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
fn the_code_licence_list_is_the_25_libraries_read_for_the_archive_without_text_to_speech() {
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
/// backend code (`AGENTS.md`): Apache-2.0, MIT or more permissive.
const INSIDE_THE_RULE: [&str; 6] = [
    "Apache-2.0",
    "MIT",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "BSL-1.0",
    "Zlib",
];

/// The entries outside that rule the owner admitted, each only as built into the upstream
/// archives the build fetches (`AGENTS.md`, the owner's decision of 2026-11-27): Eigen, and the
/// toolchain pieces under their runtime and linking exceptions.
const ADMITTED_OUTSIDE_THE_RULE: [(&str, &str); 4] = [
    ("Eigen", "MPL-2.0"),
    ("Eigen in onnxruntime", "MPL-2.0"),
    (
        "libstdc++ and libgcc",
        "GPL-3.0-or-later WITH GCC-exception-3.1",
    ),
    ("glibc", "LGPL-2.1-or-later"),
];

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
        .filter(|entry| !ADMITTED_OUTSIDE_THE_RULE.contains(entry))
        .collect();
    assert!(
        outside.is_empty(),
        "every entry is under {INSIDE_THE_RULE:?} alone or admitted by the owner as \
         {ADMITTED_OUTSIDE_THE_RULE:?}; not so: {outside:?}"
    );
}

#[test]
fn the_code_licence_constant_is_the_list_joined() {
    let joined = CODE_LICENSES
        .iter()
        .map(|entry| format!("{} ({})", entry.license, entry.component))
        .collect::<Vec<_>>()
        .join(", ");
    assert_eq!(
        CODE_LICENSE, joined,
        "CODE_LICENSE is `<licence> (<component>)` for each entry of CODE_LICENSES, joined by \
         `, `"
    );
}
