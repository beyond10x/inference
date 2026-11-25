//! Adversary pass 2 on `story:local-tts-adapter-under-licence-policy`: the paths the constructors
//! check by their metadata before anything is opened, at the kinds the unit's own cases leave
//! out: a link to a FIFO and to a device, a link that leads back to itself, a tree that links
//! back into itself, a file that may not be read, and a regular file far larger than any
//! checkpoint. Needs no weights; every case runs on the sherpa-onnx library built without
//! text-to-speech.

use std::fs;
use std::io;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

use inference_sherpa_onnx_tts::{
    ESPEAK_NG_DATA, FILES, NOT_REGULAR, OpenRefused, SherpaOnnxSynthesiser, TOKENS, tree_sha256,
};

use crate::support::scratch;

/// How long a constructor or the tree walk may take over a directory of a few small files before
/// a case says it did not answer. The threshold is this pass's, not a documented one.
const AT_ONCE: Duration = Duration::from_secs(3);

/// A fresh directory `name` for this module.
fn directory(name: &str) -> PathBuf {
    scratch(&format!("adversary2-{name}"))
}

/// Writes every pinned file but the model into `dir`, each with other content than its pin, and
/// the phoneme data directory with one file. Returns the path of the model, which is not there.
fn all_but_the_model(dir: &Path) -> PathBuf {
    for file in &FILES[1..] {
        fs::write(dir.join(file.name), b"not the pinned file").expect("the file is written");
    }
    let data = dir.join(ESPEAK_NG_DATA.name);
    fs::create_dir_all(&data).expect("the directory is created");
    fs::write(data.join("phontab"), b"not phoneme data").expect("the file is written");
    dir.join(FILES[0].name)
}

/// What `call` answers within [`AT_ONCE`], on a thread of its own; none when it has not answered
/// by then. The thread is left behind in that case and ends with the test's process.
fn answered<T: Send + 'static>(call: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (answer, answers) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = answer.send(call());
    });
    answers.recv_timeout(AT_ONCE).ok()
}

fn refusal(opened: Result<SherpaOnnxSynthesiser, OpenRefused>) -> OpenRefused {
    opened.map(|_| ()).expect_err("the directory is refused")
}

#[test]
fn a_pinned_file_that_is_a_link_to_a_fifo_or_to_a_device_is_refused_without_being_opened() {
    let pipes = directory("link-targets");
    let pipe = pipes.join("pipe");
    let made = Command::new("mkfifo")
        .arg(&pipe)
        .status()
        .expect("mkfifo is run");
    assert!(made.success(), "mkfifo {}", pipe.display());

    for (name, target) in [("fifo", pipe), ("device", PathBuf::from("/dev/zero"))] {
        let dir = directory(&format!("link-to-{name}"));
        let model = all_but_the_model(&dir);
        symlink(&target, &model).expect("the link is made");
        let refused = answered(move || refusal(SherpaOnnxSynthesiser::open(&dir)));
        assert_eq!(
            refused,
            Some(OpenRefused::FileMissing {
                path: model,
                kind: NOT_REGULAR,
            }),
            "a link to a {name} in the place of the model: what a link points to counts, and it \
             is refused by its metadata within {AT_ONCE:?}"
        );
    }
}

#[test]
fn a_link_that_leads_back_to_itself_is_refused_by_both_constructors() {
    let dir = directory("loop-pinned");
    let model = all_but_the_model(&dir);
    symlink(&model, &model).expect("the link is made");
    let refused = {
        let dir = dir.clone();
        answered(move || refusal(SherpaOnnxSynthesiser::open(&dir)))
    };
    assert!(
        matches!(&refused, Some(OpenRefused::FileMissing { path, .. }) if *path == model),
        "a model that is a link to itself is refused naming it: {refused:?}"
    );

    let dir = directory("loop-voice");
    let model = dir.join("voice.onnx");
    symlink(&model, &model).expect("the link is made");
    fs::write(dir.join(TOKENS), b"_ 0\n").expect("the file is written");
    fs::create_dir_all(dir.join("espeak-ng-data")).expect("the directory is created");
    let refused = {
        let dir = dir.clone();
        answered(move || refusal(SherpaOnnxSynthesiser::open_voice(&dir)))
    };
    assert!(
        matches!(
            &refused,
            Some(OpenRefused::FileMissing { path, .. }) if *path == model
        ) || matches!(
            &refused,
            Some(OpenRefused::VoiceModelNotSingle { found: 0, .. })
        ),
        "a voice whose model is a link to itself is refused, as a missing file or as no model: \
         {refused:?}"
    );
}

#[test]
fn a_tree_that_links_back_into_itself_is_refused_and_its_walk_ends() {
    let dir = directory("tree-loop");
    fs::create_dir_all(dir.join("a")).expect("the directory is created");
    fs::write(dir.join("a").join("file"), b"first").expect("the file is written");
    fs::write(dir.join("b"), b"second").expect("the file is written");
    symlink("..", dir.join("a").join("up")).expect("the link is made");
    symlink(".", dir.join("again")).expect("the link is made");
    symlink(".", dir.join("once-more")).expect("the link is made");

    let walked = answered(move || tree_sha256(&dir).map_err(|error| error.kind()));
    assert!(
        matches!(walked, Some(Err(_))),
        "a tree with links to its own directories has no end: the walk is refused within \
         {AT_ONCE:?} and reads no file twice over: {walked:?}"
    );
}

#[test]
fn a_pinned_file_that_may_not_be_read_is_refused_as_not_readable() {
    let dir = directory("unreadable");
    let model = all_but_the_model(&dir);
    fs::write(&model, b"not the pinned file").expect("the file is written");
    fs::set_permissions(&model, fs::Permissions::from_mode(0o000))
        .expect("the permissions are set");
    if fs::File::open(&model).is_ok() {
        eprintln!("this process reads a file of mode 000: the case has nothing to assert");
        return;
    }
    let refused = {
        let dir = dir.clone();
        answered(move || refusal(SherpaOnnxSynthesiser::open(&dir)))
    };
    fs::set_permissions(&model, fs::Permissions::from_mode(0o600))
        .expect("the permissions are set back");
    assert_eq!(
        refused,
        Some(OpenRefused::FileMissing {
            path: model,
            kind: io::ErrorKind::PermissionDenied,
        })
    );
}

/// `NOT_REGULAR` is documented with its reason: "reading one can block without end, so it is
/// never opened". A regular file is opened and hashed to its end whatever its length, though the
/// pin is of one file of one length: a sparse file of 64 GiB in the place of the model keeps
/// `open` reading for as long as 64 GiB take. The case creates no data block and removes the file
/// before it asserts.
#[test]
fn a_regular_file_far_larger_than_the_pinned_one_is_refused_without_being_read_to_its_end() {
    const LENGTH: u64 = 64 << 30;
    let dir = directory("sparse");
    let model = all_but_the_model(&dir);
    let file = fs::File::create(&model).expect("the file is created");
    file.set_len(LENGTH)
        .expect("the file system holds a sparse file of 64 GiB");
    drop(file);
    assert_eq!(
        fs::metadata(&model).expect("the file is there").len(),
        LENGTH
    );

    let refused = {
        let dir = dir.clone();
        answered(move || refusal(SherpaOnnxSynthesiser::open(&dir)))
    };
    fs::remove_file(&model).expect("the sparse file is removed");
    assert!(
        refused.is_some(),
        "open did not answer within {AT_ONCE:?} for a directory whose {} is a regular file of \
         {LENGTH} bytes: no length is pinned, so a file of any length is read to its end before \
         its sha256 is compared",
        FILES[0].name
    );
}
