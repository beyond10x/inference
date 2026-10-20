//! The constructor's refusals against a temporary directory of missing or wrong files: each
//! names the file, and nothing is declared. Needs no weights.
#![cfg(feature = "stt")]

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use inference_sherpa_onnx::stt::{FILES, FileRole, OpenRefused, SherpaOnnxRecogniser};
use inference_stt::inference_core::model::NonPositiveConcurrency;

/// A fresh, empty directory under Cargo's temporary directory for this test target.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("inference-sherpa-onnx-stt-open")
        .join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).expect("the previous run's directory is removed");
    }
    fs::create_dir_all(&dir).expect("the directory is created");
    dir
}

fn write_all_but(dir: &Path, skipped: Option<FileRole>) {
    for file in FILES {
        if Some(file.role) != skipped {
            fs::write(dir.join(file.name), b"not the pinned file").expect("the file is written");
        }
    }
}

/// The sha256 of `not the pinned file`.
const WRONG_SHA256: &str = "a08d9e349848bd89daff2c27d89e8eb3529dc848e2c9db433a674b629dc65d71";

fn file_name(role: FileRole) -> &'static str {
    FILES
        .iter()
        .find(|file| file.role == role)
        .expect("every role is pinned")
        .name
}

#[test]
fn an_empty_directory_is_refused_naming_the_first_missing_file() {
    let dir = scratch("empty");
    let refused = SherpaOnnxRecogniser::open(&dir).expect_err("nothing is there");
    assert_eq!(
        refused,
        OpenRefused::FileMissing {
            path: dir.join(file_name(FileRole::Encoder)),
            kind: io::ErrorKind::NotFound,
        }
    );
    assert!(
        refused.to_string().contains(file_name(FileRole::Encoder)),
        "{refused}"
    );
}

#[test]
fn every_pinned_file_is_required() {
    for role in [
        FileRole::Encoder,
        FileRole::Decoder,
        FileRole::Joiner,
        FileRole::Tokens,
    ] {
        let dir = scratch(&format!("without-{role:?}"));
        write_all_but(&dir, Some(role));
        assert_eq!(
            SherpaOnnxRecogniser::open(&dir).map(|_| ()),
            Err(OpenRefused::FileMissing {
                path: dir.join(file_name(role)),
                kind: io::ErrorKind::NotFound,
            }),
            "{role:?} missing"
        );
    }
}

#[test]
fn a_directory_in_place_of_a_file_is_refused_as_missing() {
    let dir = scratch("directory-in-place");
    write_all_but(&dir, Some(FileRole::Tokens));
    fs::create_dir(dir.join(file_name(FileRole::Tokens))).expect("the directory is created");
    let refused = SherpaOnnxRecogniser::open(&dir).map(|_| ());
    assert!(
        matches!(
            &refused,
            Err(OpenRefused::FileMissing { path, .. }) if *path == dir.join(file_name(FileRole::Tokens))
        ),
        "{refused:?}"
    );
}

#[test]
fn a_file_whose_sha256_differs_from_the_pin_is_refused_naming_it() {
    let dir = scratch("wrong-files");
    write_all_but(&dir, None);
    let refused = SherpaOnnxRecogniser::open(&dir).expect_err("no file is the pinned one");
    let encoder = FILES[0];
    assert_eq!(encoder.role, FileRole::Encoder);
    assert_eq!(
        refused,
        OpenRefused::ChecksumMismatch {
            path: dir.join(encoder.name),
            expected: encoder.sha256,
            actual: WRONG_SHA256.to_owned(),
        }
    );
    let message = refused.to_string();
    assert!(message.contains(encoder.name), "{message}");
    assert!(message.contains(encoder.sha256), "{message}");
    assert!(message.contains(WRONG_SHA256), "{message}");
}

#[test]
fn a_non_positive_maximum_concurrency_is_refused_before_any_file_is_read() {
    let dir = scratch("concurrency");
    assert_eq!(
        SherpaOnnxRecogniser::open_with(&dir, 0).map(|_| ()),
        Err(OpenRefused::NonPositiveConcurrency(
            NonPositiveConcurrency { submitted: 0 }
        ))
    );
    assert_eq!(
        SherpaOnnxRecogniser::open_with(&dir, -4).map(|_| ()),
        Err(OpenRefused::NonPositiveConcurrency(
            NonPositiveConcurrency { submitted: -4 }
        ))
    );
}
