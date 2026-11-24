//! The constructors' refusals against temporary directories of missing or wrong files, and
//! against the native library built without text-to-speech: each names its cause, none panics,
//! and nothing is declared. Needs no weights.

use std::fs;
use std::io;
use std::path::Path;

use inference_sherpa_onnx_tts::inference_tts::inference_core::model::NonPositiveConcurrency;
use inference_sherpa_onnx_tts::{
    ESPEAK_NG_DATA, FILES, FileRole, NATIVE_LIBRARY, NativeTts, OpenRefused, SherpaOnnxSynthesiser,
    TOKENS, native_tts, tree_sha256,
};

use crate::support::scratch;

/// The sha256 of `not the pinned file`.
const WRONG_SHA256: &str = "a08d9e349848bd89daff2c27d89e8eb3529dc848e2c9db433a674b629dc65d71";

fn file_name(role: FileRole) -> &'static str {
    FILES
        .iter()
        .find(|file| file.role == role)
        .expect("every role is pinned")
        .name
}

/// Writes every pinned file but `skipped` into `dir`, each with other content than the pin, and
/// the phoneme data directory with one file.
fn write_all_but(dir: &Path, skipped: Option<FileRole>) {
    for file in FILES {
        if Some(file.role) != skipped {
            fs::write(dir.join(file.name), b"not the pinned file").expect("the file is written");
        }
    }
    let data = dir.join(ESPEAK_NG_DATA.name);
    fs::create_dir_all(&data).expect("the directory is created");
    fs::write(data.join("phontab"), b"not phoneme data").expect("the file is written");
}

/// A directory in the shape of a caller's own voice whose files are not a voice.
fn voice(name: &str) -> std::path::PathBuf {
    let dir = scratch(name);
    fs::write(dir.join("voice.onnx"), b"not a model").expect("the file is written");
    fs::write(dir.join(TOKENS), b"_ 0\n").expect("the file is written");
    fs::create_dir_all(dir.join("espeak-ng-data")).expect("the directory is created");
    dir
}

fn refusal(opened: Result<SherpaOnnxSynthesiser, OpenRefused>) -> OpenRefused {
    opened.map(|_| ()).expect_err("the directory is refused")
}

#[test]
fn an_empty_directory_is_refused_naming_the_first_missing_file() {
    let dir = scratch("empty");
    let refused = refusal(SherpaOnnxSynthesiser::open(&dir));
    assert_eq!(
        refused,
        OpenRefused::FileMissing {
            path: dir.join(file_name(FileRole::Model)),
            kind: io::ErrorKind::NotFound,
        }
    );
    assert!(
        refused.to_string().contains(file_name(FileRole::Model)),
        "{refused}"
    );
}

#[test]
fn every_pinned_file_is_required() {
    for role in [FileRole::Model, FileRole::Voices, FileRole::Tokens] {
        let dir = scratch(&format!("without-{role:?}"));
        write_all_but(&dir, Some(role));
        assert_eq!(
            refusal(SherpaOnnxSynthesiser::open(&dir)),
            OpenRefused::FileMissing {
                path: dir.join(file_name(role)),
                kind: io::ErrorKind::NotFound,
            },
            "{role:?} missing"
        );
    }
}

#[test]
fn the_phoneme_data_directory_is_required_and_is_a_directory() {
    let dir = scratch("without-data");
    write_all_but(&dir, None);
    let data = dir.join(ESPEAK_NG_DATA.name);
    fs::remove_dir_all(&data).expect("the directory is removed");
    assert_eq!(
        refusal(SherpaOnnxSynthesiser::open(&dir)),
        OpenRefused::FileMissing {
            path: data.clone(),
            kind: io::ErrorKind::NotFound,
        }
    );
    fs::write(&data, b"a file").expect("the file is written");
    assert_eq!(
        refusal(SherpaOnnxSynthesiser::open(&dir)),
        OpenRefused::FileMissing {
            path: data,
            kind: io::ErrorKind::NotADirectory,
        }
    );
}

#[test]
fn a_file_that_is_not_the_pinned_one_is_refused_with_both_checksums() {
    let dir = scratch("wrong");
    write_all_but(&dir, None);
    let refused = refusal(SherpaOnnxSynthesiser::open(&dir));
    let model = FILES[0];
    assert_eq!(
        refused,
        OpenRefused::ChecksumMismatch {
            path: dir.join(model.name),
            expected: model.sha256,
            actual: WRONG_SHA256.to_owned(),
        }
    );
    let message = refused.to_string();
    assert!(
        message.contains(model.sha256) && message.contains(WRONG_SHA256),
        "{message}"
    );
}

#[test]
fn a_maximum_concurrency_below_one_is_refused_before_any_file_is_read() {
    let dir = scratch("never-read").join("not-there");
    for submitted in [0, -3] {
        let refused = OpenRefused::NonPositiveConcurrency(NonPositiveConcurrency { submitted });
        assert_eq!(
            refusal(SherpaOnnxSynthesiser::open_with(&dir, submitted)),
            refused
        );
        assert_eq!(
            refusal(SherpaOnnxSynthesiser::open_voice_with(&dir, submitted)),
            refused
        );
    }
}

#[test]
fn the_digest_of_a_tree_is_that_of_its_sorted_sha256sum_lines() {
    let dir = scratch("tree");
    fs::create_dir_all(dir.join("a")).expect("the directory is created");
    fs::write(dir.join("b"), b"first").expect("the file is written");
    fs::write(dir.join("a").join("c d"), b"second").expect("the file is written");
    fs::write(dir.join("a").join("B"), b"third").expect("the file is written");
    // `find * -type f -print0 | LC_ALL=C sort -z | xargs -0 sha256sum | sha256sum` in the tree.
    assert_eq!(
        tree_sha256(&dir).expect("the tree is read"),
        (
            3,
            "c2c5bb37285e04c2a71102631b41416538af5b1c42a8a7cd5f01a1a6ce4aa99d".to_owned()
        )
    );
    fs::write(dir.join("a").join("B"), b"changed").expect("the file is written");
    assert_ne!(
        tree_sha256(&dir).expect("the tree is read").1,
        "c2c5bb37285e04c2a71102631b41416538af5b1c42a8a7cd5f01a1a6ce4aa99d"
    );
}

#[test]
fn a_voice_directory_that_is_not_there_is_refused_naming_it() {
    let dir = scratch("no-voice").join("not-there");
    assert_eq!(
        refusal(SherpaOnnxSynthesiser::open_voice(&dir)),
        OpenRefused::FileMissing {
            path: dir,
            kind: io::ErrorKind::NotFound,
        }
    );
}

#[test]
fn a_voice_directory_holds_exactly_one_model() {
    let dir = voice("voice-models");
    fs::remove_file(dir.join("voice.onnx")).expect("the file is removed");
    assert_eq!(
        refusal(SherpaOnnxSynthesiser::open_voice(&dir)),
        OpenRefused::VoiceModelNotSingle {
            dir: dir.clone(),
            found: 0,
        }
    );
    fs::write(dir.join("one.onnx"), b"not a model").expect("the file is written");
    fs::write(dir.join("two.onnx"), b"not a model").expect("the file is written");
    let refused = refusal(SherpaOnnxSynthesiser::open_voice(&dir));
    assert_eq!(
        refused,
        OpenRefused::VoiceModelNotSingle {
            dir: dir.clone(),
            found: 2,
        }
    );
    assert!(refused.to_string().contains("2 *.onnx files"), "{refused}");
}

#[test]
fn a_voice_directory_needs_its_tokens_and_its_phoneme_data() {
    let dir = voice("voice-without-tokens");
    fs::remove_file(dir.join(TOKENS)).expect("the file is removed");
    assert_eq!(
        refusal(SherpaOnnxSynthesiser::open_voice(&dir)),
        OpenRefused::FileMissing {
            path: dir.join(TOKENS),
            kind: io::ErrorKind::NotFound,
        }
    );
    let dir = voice("voice-without-data");
    fs::remove_dir_all(dir.join("espeak-ng-data")).expect("the directory is removed");
    assert_eq!(
        refusal(SherpaOnnxSynthesiser::open_voice(&dir)),
        OpenRefused::FileMissing {
            path: dir.join("espeak-ng-data"),
            kind: io::ErrorKind::NotFound,
        }
    );
}

/// The library the tests that are not ignored run on: the one built without text-to-speech,
/// which the workspace builds against.
fn library_without_tts() -> std::path::PathBuf {
    match native_tts() {
        NativeTts::Absent { library } => library,
        other => panic!(
            "the tests that are not ignored run on the sherpa-onnx library built without \
             text-to-speech, but this process loaded {other:?}"
        ),
    }
}

#[test]
fn the_library_the_workspace_builds_against_is_read_as_built_without_tts() {
    let library = library_without_tts();
    assert!(
        library
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(NATIVE_LIBRARY)),
        "{}",
        library.display()
    );
    assert!(library.is_file(), "{}", library.display());
}

#[test]
fn opening_a_voice_on_a_library_without_tts_is_refused_naming_the_library() {
    let library = library_without_tts();
    let dir = voice("voice-no-tts");
    let refused = refusal(SherpaOnnxSynthesiser::open_voice(&dir));
    assert_eq!(
        refused,
        OpenRefused::LibraryWithoutTts {
            library: library.clone()
        }
    );
    let message = refused.to_string();
    assert!(
        message.contains("built without text-to-speech")
            && message.contains(&library.display().to_string()),
        "{message}"
    );
}

#[test]
#[ignore = "needs INFERENCE_SHERPA_ONNX_TTS_DIR; run on the sherpa-onnx library built without text-to-speech"]
fn opening_the_pinned_checkpoint_on_a_library_without_tts_is_refused_naming_the_library() {
    let library = library_without_tts();
    assert_eq!(
        refusal(SherpaOnnxSynthesiser::open(crate::support::checkpoint_dir())),
        OpenRefused::LibraryWithoutTts { library }
    );
}

#[test]
#[ignore = "needs INFERENCE_SHERPA_ONNX_TTS_DIR and the sherpa-onnx library built with text-to-speech"]
fn the_pinned_checkpoint_opens_and_its_phoneme_data_is_the_pinned_tree() {
    assert!(
        matches!(native_tts(), NativeTts::Present { .. }),
        "{:?}",
        native_tts()
    );
    let dir = crate::support::checkpoint_dir();
    let (files, sha256) =
        tree_sha256(&dir.join(ESPEAK_NG_DATA.name)).expect("the phoneme data is read");
    eprintln!("{}: {files} files, sha256 {sha256}", ESPEAK_NG_DATA.name);
    assert_eq!(
        (files, sha256.as_str()),
        (ESPEAK_NG_DATA.files, ESPEAK_NG_DATA.sha256)
    );
    let synthesiser = crate::support::open(1);
    assert_eq!(synthesiser.speakers(), 11);
    assert_eq!(synthesiser.format().sample_rate.get(), 24_000);
    assert!(synthesiser.clone().with_speaker(10).is_ok());
    assert!(synthesiser.with_speaker(11).is_err());
}
