//! The native code this crate links carries no GPL-licensed phonemiser: no espeak-ng and no
//! piper-phonemize symbol is in the test binary or in any sherpa-onnx or onnxruntime library it
//! loaded. sherpa-onnx's default build turns text-to-speech on and links both, so a version bump
//! or an archive swap that brings them back fails here. Needs no weights.
#![cfg(all(feature = "stt", target_os = "linux"))]

use std::fs;
use std::path::PathBuf;

use inference_sherpa_onnx::stt::{NATIVE_VERSION, native_version};

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
