//! Which sherpa-onnx native library the process loaded, and whether it was built with
//! text-to-speech.
//!
//! sherpa-onnx publishes two builds of its shared library under one file name and one set of
//! exported symbols: one with text-to-speech, which carries espeak-ng, and one without, whose
//! text-to-speech functions print a line and return nothing. A crate links against either. Which
//! one a process runs on is the loader's choice when the process starts, so it is read here from
//! the process's own memory map.

use std::fs;
use std::path::{Path, PathBuf};

/// The file name of the sherpa-onnx native library, as the loader maps it.
pub const NATIVE_LIBRARY: &str = "libsherpa-onnx-c-api.so";

/// The message the build of the native library without text-to-speech prints from its
/// text-to-speech functions. The build with text-to-speech does not hold it.
const WITHOUT_TTS: &[u8] = b"TTS is not enabled";

/// Whether the sherpa-onnx native library the process loaded can speak.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeTts {
    /// The library at `library` was built with text-to-speech.
    Present { library: PathBuf },
    /// The library at `library` was built without text-to-speech: it creates no synthesiser.
    Absent { library: PathBuf },
    /// The process's memory map names no [`NATIVE_LIBRARY`], or it or the library cannot be
    /// read: on a system without `/proc/self/maps`, or when the library is linked statically.
    Unknown,
}

/// Reads which [`NATIVE_LIBRARY`] the process loaded and whether it was built with
/// text-to-speech.
#[must_use]
pub fn native_tts() -> NativeTts {
    let Some(library) = loaded_library() else {
        return NativeTts::Unknown;
    };
    match fs::read(&library) {
        Ok(bytes) if holds(&bytes, WITHOUT_TTS) => NativeTts::Absent { library },
        Ok(_) => NativeTts::Present { library },
        Err(_) => NativeTts::Unknown,
    }
}

/// The path of the [`NATIVE_LIBRARY`] in the process's memory map.
fn loaded_library() -> Option<PathBuf> {
    let maps = fs::read_to_string("/proc/self/maps").ok()?;
    maps.lines().find_map(mapped_library)
}

/// The path a line of `/proc/self/maps` maps, when it is a [`NATIVE_LIBRARY`]: the sixth field,
/// which runs to the end of the line and may hold spaces.
fn mapped_library(line: &str) -> Option<PathBuf> {
    let mut rest = line;
    for _ in 0..5 {
        rest = rest.trim_start();
        rest = &rest[rest.find(' ')?..];
    }
    let path = Path::new(rest.trim_start());
    let name = path.file_name()?.to_str()?;
    name.starts_with(NATIVE_LIBRARY).then(|| path.to_path_buf())
}

/// Whether `bytes` holds `needle`.
fn holds(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.windows(needle.len()).any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_of_the_memory_map_names_the_library_it_maps() {
        let line = "7f1c2a400000-7f1c2a52b000 r-xp 00012000 fd:01 1315 \
                    /opt/with space/lib/libsherpa-onnx-c-api.so";
        assert_eq!(
            mapped_library(line),
            Some(PathBuf::from("/opt/with space/lib/libsherpa-onnx-c-api.so"))
        );
    }

    #[test]
    fn other_mappings_are_not_the_library() {
        for line in [
            "7f1c2a400000-7f1c2a52b000 r-xp 00012000 fd:01 1315 /usr/lib/libonnxruntime.so",
            "7ffd3c1f2000-7ffd3c213000 rw-p 00000000 00:00 0 [stack]",
            "7f1c2a400000-7f1c2a52b000 rw-p 00000000 00:00 0",
            "",
        ] {
            assert_eq!(mapped_library(line), None, "{line:?}");
        }
    }

    #[test]
    fn the_marker_is_found_anywhere_in_the_bytes() {
        assert!(holds(b"..TTS is not enabled. Please rebuild", WITHOUT_TTS));
        assert!(!holds(b"OfflineTtsConfig(", WITHOUT_TTS));
        assert!(!holds(b"TTS", WITHOUT_TTS));
    }
}
