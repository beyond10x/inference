//! The constructors' refusals against temporary directories of missing or wrong files, and
//! against the native library built without text-to-speech: each names its cause, and nothing is
//! declared. Each of them comes before sherpa-onnx is called, so none ends the process. A voice
//! file that passes these checks and that sherpa-onnx cannot load does end it, inside the native
//! load: `open_voice_with` documents that, and the boundary cases of the offline cascade example
//! show it in a child process. Needs no weights.

use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use inference_sherpa_onnx_tts::inference_tts::inference_core::model::NonPositiveConcurrency;
use inference_sherpa_onnx_tts::{
    ESPEAK_NG_DATA, FILES, FileRole, NATIVE_LIBRARY, NOT_REGULAR, NativeTts, OpenRefused,
    SherpaOnnxSynthesiser, TOKENS, native_tts, tree_sha256,
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

/// Makes a FIFO at `path`: a path that is neither a regular file nor a directory, and whose open
/// for reading waits until something writes.
fn fifo(path: &Path) {
    let made = Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("mkfifo is run");
    assert!(made.success(), "mkfifo {}", path.display());
}

/// What `open` answers, on a thread of its own: a constructor that opened a FIFO would wait
/// there, and the case fails after 10 s instead of waiting with it.
fn answered<T: Send + 'static>(open: impl FnOnce() -> T + Send + 'static) -> T {
    let (answer, answers) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = answer.send(open());
    });
    answers
        .recv_timeout(Duration::from_secs(10))
        .expect("the constructor answers at once: it opens no path that is not a regular file")
}

#[test]
fn a_pinned_file_that_is_not_a_regular_file_is_refused_by_its_metadata_without_being_opened() {
    for role in [FileRole::Model, FileRole::Voices, FileRole::Tokens] {
        let dir = scratch(&format!("fifo-{role:?}"));
        write_all_but(&dir, Some(role));
        let path = dir.join(file_name(role));
        fifo(&path);
        let refused = answered(move || refusal(SherpaOnnxSynthesiser::open(&dir)));
        assert_eq!(
            refused,
            OpenRefused::FileMissing {
                path,
                kind: NOT_REGULAR,
            },
            "{role:?} as a FIFO"
        );
    }
}

#[test]
fn a_pinned_file_reached_through_a_symbolic_link_counts_as_the_file_it_points_to() {
    let dir = scratch("linked");
    write_all_but(&dir, Some(FileRole::Model));
    let elsewhere = scratch("linked-to").join("a-model");
    fs::write(&elsewhere, b"not the pinned file").expect("the file is written");
    let model = dir.join(file_name(FileRole::Model));
    symlink(&elsewhere, &model).expect("the link is made");
    assert_eq!(
        refusal(SherpaOnnxSynthesiser::open(&dir)),
        OpenRefused::ChecksumMismatch {
            path: model.clone(),
            expected: FILES[0].sha256,
            actual: WRONG_SHA256.to_owned(),
        },
        "the link to a regular file is read and hashed"
    );
    fs::remove_file(&elsewhere).expect("the file is removed");
    assert_eq!(
        refusal(SherpaOnnxSynthesiser::open(&dir)),
        OpenRefused::FileMissing {
            path: model,
            kind: io::ErrorKind::NotFound,
        },
        "a link to nothing is a missing file"
    );
}

#[test]
fn a_tree_entry_that_is_neither_a_regular_file_nor_a_directory_is_refused_without_being_opened() {
    let dir = scratch("tree-with-fifo");
    fs::create_dir_all(dir.join("a")).expect("the directory is created");
    fs::write(dir.join("b"), b"first").expect("the file is written");
    let linked = scratch("tree-linked-to").join("c");
    fs::write(&linked, b"second").expect("the file is written");
    symlink(&linked, dir.join("a").join("c")).expect("the link is made");
    let (files, _) = {
        let dir = dir.clone();
        answered(move || tree_sha256(&dir)).expect("a tree of files and links to files is read")
    };
    assert_eq!(files, 2, "the link to a regular file counts as a file");

    fifo(&dir.join("a").join("pipe"));
    let refused = answered(move || tree_sha256(&dir)).expect_err("the tree is refused");
    assert_eq!(refused.kind(), NOT_REGULAR);
    assert!(refused.to_string().contains("a/pipe"), "{refused}");
}

#[test]
fn a_voice_file_that_is_empty_or_not_a_regular_file_is_refused_before_sherpa_onnx_is_called() {
    // The library these cases run on has no text-to-speech: each refusal below comes before the
    // one that names the library, so before any call into sherpa-onnx.
    library_without_tts();

    let dir = voice("voice-empty-model");
    let model = dir.join("voice.onnx");
    fs::write(&model, b"").expect("the file is written");
    let refused = refusal(SherpaOnnxSynthesiser::open_voice(&dir));
    assert_eq!(
        refused,
        OpenRefused::FileEmpty {
            path: model.clone()
        }
    );
    assert!(refused.to_string().contains("is empty"), "{refused}");

    let dir = voice("voice-empty-tokens");
    fs::write(dir.join(TOKENS), b"").expect("the file is written");
    assert_eq!(
        refusal(SherpaOnnxSynthesiser::open_voice(&dir)),
        OpenRefused::FileEmpty {
            path: dir.join(TOKENS)
        }
    );

    let dir = voice("voice-fifo-model");
    let model = dir.join("voice.onnx");
    fs::remove_file(&model).expect("the file is removed");
    fifo(&model);
    let refused = answered(move || refusal(SherpaOnnxSynthesiser::open_voice(&dir)));
    assert_eq!(
        refused,
        OpenRefused::FileMissing {
            path: model,
            kind: NOT_REGULAR,
        }
    );

    let dir = voice("voice-fifo-tokens");
    let tokens = dir.join(TOKENS);
    fs::remove_file(&tokens).expect("the file is removed");
    fifo(&tokens);
    let refused = answered(move || refusal(SherpaOnnxSynthesiser::open_voice(&dir)));
    assert_eq!(
        refused,
        OpenRefused::FileMissing {
            path: tokens,
            kind: NOT_REGULAR,
        }
    );
}
