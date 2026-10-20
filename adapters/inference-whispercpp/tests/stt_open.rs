//! The constructor's refusals against a temporary file that is missing, not a file or not the
//! pinned one: each names the file, and nothing is declared. whisper.cpp's own refusal to load a
//! file cannot be reached without one carrying the pinned hash, so only its message is checked.
//! Needs no weights.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use inference_stt::inference_core::model::NonPositiveConcurrency;
use inference_whispercpp::stt::{FILE_NAME, OpenRefused, SHA256, WhisperCppRecogniser};

/// A fresh, empty directory under Cargo's temporary directory for this test target.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("inference-whispercpp-stt-open")
        .join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).expect("the previous run's directory is removed");
    }
    fs::create_dir_all(&dir).expect("the directory is created");
    dir
}

/// The sha256 of `not the pinned file`.
const WRONG_SHA256: &str = "a08d9e349848bd89daff2c27d89e8eb3529dc848e2c9db433a674b629dc65d71";

#[test]
fn a_missing_file_is_refused_naming_it() {
    let path = scratch("missing").join(FILE_NAME);
    let refused = WhisperCppRecogniser::open(&path).expect_err("nothing is there");
    assert_eq!(
        refused,
        OpenRefused::FileMissing {
            path: path.clone(),
            kind: io::ErrorKind::NotFound,
        }
    );
    assert!(
        refused.to_string().contains(&path.display().to_string()),
        "{refused}"
    );
}

#[test]
fn a_directory_in_place_of_the_file_is_refused_as_missing() {
    let path = scratch("directory-in-place").join(FILE_NAME);
    fs::create_dir(&path).expect("the directory is created");
    let refused = WhisperCppRecogniser::open(&path).map(|_| ());
    assert!(
        matches!(&refused, Err(OpenRefused::FileMissing { path: refused, .. }) if *refused == path),
        "{refused:?}"
    );
}

#[test]
fn a_file_whose_sha256_differs_from_the_pin_is_refused_naming_it() {
    let path = scratch("wrong-file").join(FILE_NAME);
    fs::write(&path, b"not the pinned file").expect("the file is written");
    let refused = WhisperCppRecogniser::open(&path).expect_err("the file is not the pinned one");
    assert_eq!(
        refused,
        OpenRefused::ChecksumMismatch {
            path: path.clone(),
            expected: SHA256,
            actual: WRONG_SHA256.to_owned(),
        }
    );
    let message = refused.to_string();
    assert!(message.contains(&path.display().to_string()), "{message}");
    assert!(message.contains(SHA256), "{message}");
    assert!(message.contains(WRONG_SHA256), "{message}");
}

#[test]
fn a_non_positive_maximum_concurrency_is_refused_before_the_file_is_read() {
    let path = scratch("concurrency").join(FILE_NAME);
    assert_eq!(
        WhisperCppRecogniser::open_with(&path, 0).map(|_| ()),
        Err(OpenRefused::NonPositiveConcurrency(
            NonPositiveConcurrency { submitted: 0 }
        ))
    );
    assert_eq!(
        WhisperCppRecogniser::open_with(&path, -4).map(|_| ()),
        Err(OpenRefused::NonPositiveConcurrency(
            NonPositiveConcurrency { submitted: -4 }
        ))
    );
}

#[test]
fn a_backend_refusal_names_the_file() {
    let refused = OpenRefused::BackendRefused {
        path: PathBuf::from("models").join(FILE_NAME),
        detail: "the backend's own words".to_owned(),
    };
    let message = refused.to_string();
    assert!(message.contains(FILE_NAME), "{message}");
    assert!(message.contains("the backend's own words"), "{message}");
}
