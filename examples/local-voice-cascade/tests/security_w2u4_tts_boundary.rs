//! Verification pass on `story:local-tts-adapter-under-licence-policy`: the licence boundary and
//! the file boundary of `inference-sherpa-onnx-tts`, checked from the one package that depends on
//! it. The adapter's own package declares its tests by hand (`autotests = false`), so a new file
//! there is not built; these cases can move into its `tests/main.rs` as a module.
//!
//! The cases that are not ignored need no weights and run on the sherpa-onnx library built
//! without text-to-speech. The ignored ones need `INFERENCE_SHERPA_ONNX_TTS_DIR` and a process
//! started on the library built with text-to-speech. Nothing here downloads anything.

#![cfg(all(target_os = "linux", feature = "local"))]

use std::fs;
use std::io;
use std::os::unix::fs::{OpenOptionsExt, symlink};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::sync::mpsc;
use std::time::Duration;

use inference_sherpa_onnx_tts::{
    ARCHIVE_SHA256, ARCHIVE_URL, CHECKPOINT, ESPEAK_NG_DATA, FILES, NATIVE_VERSION, NativeTts,
    OpenRefused, SherpaOnnxSynthesiser, TOKENS, native_tts, native_version,
};

/// The package name of the adapter.
const ADAPTER: &str = "inference-sherpa-onnx-tts";

/// The manifest of this package, the one dependent the adapter may have.
const CASCADE_MANIFEST: &str = "examples/local-voice-cascade/Cargo.toml";

/// The sha256 of `sherpa-onnx-v1.13.8-linux-x64-shared-lib.tar.bz2`, the native library built
/// with text-to-speech, as the story's scope read it from the release.
const NATIVE_ARCHIVE_SHA256: &str =
    "3892d184be41027e18165e67f549cd4e4cdd8dcd73ac5579e97afd55e14e30b6";

/// `O_NONBLOCK` on Linux: opening a FIFO for writing with it fails at once while nothing reads.
const O_NONBLOCK: i32 = 0o4000;

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the package is two directories below the repository")
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// A fresh, empty directory under Cargo's temporary directory for this test target.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("security-w2u4-tts-boundary")
        .join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).expect("the previous run's directory is removed");
    }
    fs::create_dir_all(&dir).expect("the directory is created");
    dir
}

// --- Files from a caller -------------------------------------------------------------------

/// `open` documents that a pinned file which "is not of that kind" is refused with
/// `FileMissing` before anything is read. A FIFO is not a file to hash: opening it blocks until
/// something writes.
#[test]
fn a_pinned_file_that_is_a_fifo_is_refused_and_is_not_opened() {
    let dir = scratch("fifo-model");
    let model = dir.join(FILES[0].name);
    let made = Command::new("mkfifo")
        .arg(&model)
        .status()
        .expect("mkfifo is run");
    assert!(made.success(), "mkfifo {}", model.display());
    for file in &FILES[1..] {
        fs::write(dir.join(file.name), b"not the pinned file").expect("the file is written");
    }
    fs::create_dir_all(dir.join(ESPEAK_NG_DATA.name)).expect("the directory is created");

    let (answer, answered) = mpsc::channel();
    let opening = {
        let dir = dir.clone();
        std::thread::spawn(move || {
            let _ = answer.send(SherpaOnnxSynthesiser::open(&dir).map(|_| ()));
        })
    };
    let answered = answered.recv_timeout(Duration::from_secs(3));
    if answered.is_err() {
        // The thread waits in the open of the FIFO. A writer that opens and closes it ends that
        // wait, so the thread is not left behind.
        for _ in 0..100 {
            let writer = fs::OpenOptions::new()
                .write(true)
                .custom_flags(O_NONBLOCK)
                .open(&model);
            if writer.is_ok() {
                drop(writer);
                let _ = opening.join();
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    match answered {
        Ok(Err(OpenRefused::FileMissing { path, .. })) => assert_eq!(path, model),
        Ok(other) => panic!(
            "a FIFO in the place of {} is answered {other:?}, not refused as not a file",
            FILES[0].name
        ),
        Err(_) => panic!(
            "open did not answer within 3 s for a directory whose {} is a FIFO: a path that is \
             neither a file nor a directory passes the check of its kind and is opened to be \
             hashed",
            FILES[0].name
        ),
    }
}

/// A path with a NUL byte cannot be given to sherpa-onnx, whose configuration takes C strings
/// and panics on one. Both constructors refuse it before sherpa-onnx is reached.
#[test]
fn a_directory_path_with_a_nul_byte_is_refused_by_both_constructors() {
    let dir = scratch("nul").join("voice\0directory");
    for opened in [
        SherpaOnnxSynthesiser::open(&dir).map(|_| ()),
        SherpaOnnxSynthesiser::open_voice(&dir).map(|_| ()),
    ] {
        assert!(
            matches!(
                opened,
                Err(OpenRefused::FileMissing {
                    kind: io::ErrorKind::InvalidInput,
                    ..
                })
            ),
            "{opened:?}"
        );
    }
}

fn checkpoint_dir() -> PathBuf {
    std::env::var_os("INFERENCE_SHERPA_ONNX_TTS_DIR")
        .map(PathBuf::from)
        .expect("INFERENCE_SHERPA_ONNX_TTS_DIR names the directory of the unpacked checkpoint")
}

/// A directory in the shape of a caller's own voice: the tokens and the phoneme data of the
/// pinned checkpoint, and the model `model` puts at `voice.onnx`.
fn voice(name: &str, model: impl FnOnce(&Path)) -> PathBuf {
    let checkpoint = checkpoint_dir();
    let dir = scratch(name);
    model(&dir.join("voice.onnx"));
    fs::copy(checkpoint.join(TOKENS), dir.join(TOKENS)).expect("the tokens are copied");
    symlink(
        checkpoint.join(ESPEAK_NG_DATA.name),
        dir.join("espeak-ng-data"),
    )
    .expect("the phoneme data is linked");
    dir
}

fn on_the_library_with_tts() {
    assert!(
        matches!(native_tts(), NativeTts::Present { .. }),
        "this case runs on the sherpa-onnx library built with text-to-speech: {:?}",
        native_tts()
    );
}

/// Set in the child process a case starts of itself: the voice directory the child opens.
const CHILD_OPENS: &str = "SECURITY_W2U4_CHILD_OPENS";

/// What the child writes to standard error when `open_voice` returned to it.
const RETURNED: &str = "open_voice returned:";

/// `SIGABRT` on Linux.
const SIGABRT: i32 = 6;

/// In the child process of a case: opens the voice the parent named and writes that the
/// constructor returned, and what. `false` in the parent, where nothing is opened.
fn child_opens() -> bool {
    let Some(dir) = std::env::var_os(CHILD_OPENS) else {
        return false;
    };
    let opened = SherpaOnnxSynthesiser::open_voice(PathBuf::from(dir)).map(|_| ());
    eprintln!("{RETURNED} {opened:?}");
    true
}

/// Runs the case `name` of this test binary again as a child process that opens the voice in
/// `dir`, on the library this process runs on, and returns how the child ended and what it wrote
/// to standard error.
fn opened_in_a_child(name: &str, dir: &Path) -> (ExitStatus, String) {
    let binary = std::env::current_exe().expect("the test binary has a path");
    let output = Command::new(binary)
        .args(["--exact", name, "--ignored", "--nocapture"])
        .env(CHILD_OPENS, dir)
        .output()
        .expect("the test binary is run again");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    eprintln!(
        "the child ended with {}; its standard error:\n{stderr}",
        output.status
    );
    (output.status, stderr)
}

/// `open_voice` documents that a voice file sherpa-onnx cannot load ends the process inside the
/// native load, where no refusal comes back. A voice whose model file is not a model, a
/// download cut short for one, aborts the process that opens it.
#[test]
#[ignore = "needs INFERENCE_SHERPA_ONNX_TTS_DIR and the sherpa-onnx library built with text-to-speech"]
fn a_voice_whose_model_file_is_not_a_model_aborts_the_process_that_opens_it() {
    if child_opens() {
        return;
    }
    on_the_library_with_tts();
    let dir = voice("voice-not-a-model", |path| {
        fs::write(path, b"not a model").expect("the file is written");
    });
    let (status, stderr) = opened_in_a_child(
        "a_voice_whose_model_file_is_not_a_model_aborts_the_process_that_opens_it",
        &dir,
    );
    assert!(
        !stderr.contains(RETURNED),
        "open_voice is documented to end the process for a model file that is not a model, but \
         it returned: the documents and this case are to say what it returns"
    );
    assert_eq!(
        status.signal(),
        Some(SIGABRT),
        "the process that opens a voice whose model file is not a model is aborted: {status}"
    );
}

/// The same for a model sherpa-onnx can load that is not a VITS voice in the form it reads:
/// here the pinned Kokoro model in the place of the voice's model. sherpa-onnx ends the process
/// with exit status 255.
#[test]
#[ignore = "needs INFERENCE_SHERPA_ONNX_TTS_DIR and the sherpa-onnx library built with text-to-speech"]
fn a_voice_whose_model_is_of_another_family_ends_the_process_that_opens_it_with_status_255() {
    if child_opens() {
        return;
    }
    on_the_library_with_tts();
    let pinned = checkpoint_dir().join(FILES[0].name);
    let dir = voice("voice-of-another-family", |path| {
        symlink(&pinned, path).expect("the model is linked");
    });
    let (status, stderr) = opened_in_a_child(
        "a_voice_whose_model_is_of_another_family_ends_the_process_that_opens_it_with_status_255",
        &dir,
    );
    assert!(
        !stderr.contains(RETURNED),
        "open_voice is documented to end the process for a model of another family, but it \
         returned: the documents and this case are to say what it returns"
    );
    assert_eq!(
        status.code(),
        Some(255),
        "sherpa-onnx ends the process that opens a voice whose model is of another family with \
         exit status 255: {status}"
    );
}

/// The checks `open_voice` makes before sherpa-onnx is called, which cost a good voice nothing:
/// a model or tokens file that holds no byte is refused, naming it, and the process lives.
#[test]
fn a_voice_whose_model_or_tokens_file_is_empty_is_refused_before_sherpa_onnx_is_called() {
    for empty in ["voice.onnx", TOKENS] {
        let dir = scratch(&format!("voice-empty-{empty}"));
        for name in ["voice.onnx", TOKENS] {
            let content: &[u8] = if name == empty { b"" } else { b"not empty" };
            fs::write(dir.join(name), content).expect("the file is written");
        }
        fs::create_dir_all(dir.join("espeak-ng-data")).expect("the directory is created");
        let opened = SherpaOnnxSynthesiser::open_voice(&dir).map(|_| ());
        assert_eq!(
            opened,
            Err(OpenRefused::FileEmpty {
                path: dir.join(empty)
            })
        );
    }
}

// --- The licence boundary ------------------------------------------------------------------

/// The symbol prefixes of the GPL-licensed phonemiser and of piper-phonemize's namespace, each
/// stored reversed, as `adapters/inference-sherpa-onnx/tests/licence.rs` stores them.
const FORBIDDEN_REVERSED: [&str; 2] = ["_kaepse", "repip5NZ_"];

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

/// Every sherpa-onnx or onnxruntime library mapped into this process.
fn mapped_native_libraries() -> Vec<PathBuf> {
    let maps = read(Path::new("/proc/self/maps"));
    let mut libraries = Vec::new();
    for line in maps.lines() {
        let Some(path) = line.split_whitespace().nth(5).map(PathBuf::from) else {
            continue;
        };
        let native = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.contains("sherpa-onnx") || name.contains("onnxruntime"));
        if native && !libraries.contains(&path) {
            libraries.push(path);
        }
    }
    libraries
}

/// A test binary of this package with feature `local`, which links the text-to-speech adapter,
/// runs in the gate on the library built without text-to-speech: no phonemiser symbol is in a
/// native library it mapped.
#[test]
fn the_cascade_with_the_synthesiser_bound_maps_no_gpl_phonemiser_in_the_gate() {
    assert_eq!(
        native_version(),
        NATIVE_VERSION,
        "the native library is loaded"
    );
    let libraries = mapped_native_libraries();
    assert!(
        !libraries.is_empty(),
        "the process maps the sherpa-onnx native library"
    );
    for library in &libraries {
        let bytes = fs::read(library).expect("a mapped library is readable");
        for reversed in FORBIDDEN_REVERSED {
            let needle: Vec<u8> = reversed.bytes().rev().collect();
            assert!(
                !contains(&bytes, &needle),
                "{} carries {}: a GPL-licensed phonemiser is loaded",
                library.display(),
                String::from_utf8_lossy(&needle)
            );
        }
    }
    assert!(
        matches!(native_tts(), NativeTts::Absent { .. }),
        "{:?}",
        native_tts()
    );
}

/// Every `Cargo.toml` below `dir`, but in build output, installed packages and hidden
/// directories.
fn manifests(dir: &Path) -> Vec<PathBuf> {
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
                directories.push(path);
            } else if name == "Cargo.toml" {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// Whether the text of a manifest names the package `name` anywhere: as a dependency key in any
/// form, in a dependency table's header, as the `package` of a renamed dependency, in a path or
/// in a feature. A longer name that only starts or ends with it does not count.
fn names(manifest: &str, name: &str) -> bool {
    let part = |character: char| character.is_ascii_alphanumeric() || "-_".contains(character);
    manifest.match_indices(name).any(|(at, _)| {
        let before = manifest[..at].chars().next_back();
        let after = manifest[at + name.len()..].chars().next();
        !before.is_some_and(part) && !after.is_some_and(part)
    })
}

#[test]
fn every_form_of_a_dependency_on_the_adapter_is_read_as_one() {
    for manifest in [
        "[dependencies]\ninference-sherpa-onnx-tts = { path = \"../x\" }\n",
        "[dependencies]\ninference-sherpa-onnx-tts={ path = \"../x\" }\n",
        "[dependencies]\n\"inference-sherpa-onnx-tts\" = { path = \"../x\" }\n",
        "[dependencies]\ninference-sherpa-onnx-tts.workspace = true\n",
        "[dependencies.inference-sherpa-onnx-tts]\npath = \"../x\"\n",
        "[dependencies]\nvoice = { package = \"inference-sherpa-onnx-tts\", path = \"../x\" }\n",
        "[features]\nspeak = [\"dep:inference-sherpa-onnx-tts\"]\n",
        "[dependencies]\nvoice = { path = \"../../adapters/inference-sherpa-onnx-tts\" }\n",
    ] {
        assert!(names(manifest, ADAPTER), "{manifest}");
    }
    for manifest in [
        "[dependencies]\ninference-sherpa-onnx = { path = \"../x\", features = [\"stt\"] }\n",
        "[dependencies]\ninference-sherpa-onnx-tts-extra = { path = \"../x\" }\n",
        "[dependencies]\nmy-inference-sherpa-onnx-tts = { path = \"../x\" }\n",
    ] {
        assert!(!names(manifest, ADAPTER), "{manifest}");
    }
}

/// The owner's exception: the adapter is its own opt-in crate, never a dependency of a contract
/// crate. No manifest but its own and this package's names it, in any form; this package takes it
/// as an optional dependency that only its feature `local` turns on, and `local` is not a default
/// feature; and the packages that depend on this one, which may turn `local` on, are examples.
#[test]
fn only_the_offline_cascade_names_the_adapter_and_only_behind_its_feature() {
    let repository = repository();
    let relative = |path: &Path| {
        path.strip_prefix(&repository)
            .expect("the manifest is in the repository")
            .to_path_buf()
    };
    let own = Path::new("adapters").join(ADAPTER).join("Cargo.toml");
    let all = manifests(&repository);
    assert!(
        all.iter().any(|path| relative(path) == own),
        "the walk reaches the adapter's own manifest"
    );

    let naming: Vec<PathBuf> = all
        .iter()
        .filter(|path| names(&read(path), ADAPTER))
        .map(|path| relative(path))
        .filter(|path| *path != own)
        .collect();
    assert_eq!(
        naming,
        [PathBuf::from(CASCADE_MANIFEST)],
        "no manifest but the offline cascade's names {ADAPTER}"
    );

    let cascade = read(&repository.join(CASCADE_MANIFEST));
    let mut section = "";
    for line in cascade.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line;
            continue;
        }
        let default_features = section == "[features]" && line.starts_with("default");
        if !names(line, ADAPTER) && !default_features {
            continue;
        }
        match section {
            "[dependencies]" => assert!(
                line.starts_with(&format!("{ADAPTER} = {{")) && line.contains("optional = true"),
                "the dependency is optional: {line}"
            ),
            "[features]" if line.starts_with("default") => assert!(
                !line.contains("\"local\"") && !names(line, ADAPTER),
                "the adapter is not on by default: {line}"
            ),
            "[features]" => assert!(
                line.starts_with("local = ["),
                "only the feature `local` turns the adapter on: {line}"
            ),
            other => panic!("{ADAPTER} is named under {other}: {line}"),
        }
    }

    let through_the_cascade: Vec<PathBuf> = all
        .iter()
        .filter(|path| relative(path) != Path::new(CASCADE_MANIFEST))
        .filter(|path| names(&read(path), "local-voice-cascade"))
        .map(|path| relative(path))
        .collect();
    assert!(
        through_the_cascade
            .iter()
            .all(|path| path.starts_with("examples")),
        "only examples depend on the offline cascade, whose feature `local` brings the adapter: \
         {through_the_cascade:?}"
    );
}

// --- The fetch task ------------------------------------------------------------------------

/// The lines of the task `name` of `Taskfile.yml`, trimmed, from its header to the next task.
fn task<'a>(taskfile: &'a str, name: &str) -> Vec<&'a str> {
    let header = format!("  {name}:");
    let is_header =
        |line: &str| line.starts_with("  ") && !line.starts_with("   ") && line.ends_with(':');
    taskfile
        .lines()
        .skip_while(|line| *line != header)
        .skip(1)
        .take_while(|line| !is_header(line))
        .map(str::trim)
        .collect()
}

fn variable<'a>(lines: &[&'a str], name: &str) -> &'a str {
    let key = format!("{name}: ");
    lines
        .iter()
        .find_map(|line| line.strip_prefix(key.as_str()))
        .unwrap_or_else(|| panic!("the task sets {name}"))
}

/// The index of the one command of `commands` that starts with `start` and holds `part`.
fn the_one(commands: &[&str], start: &str, part: &str) -> usize {
    let found: Vec<usize> = commands
        .iter()
        .enumerate()
        .filter(|(_, command)| command.starts_with(start) && command.contains(part))
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        found.len(),
        1,
        "one `{start}` command with `{part}`: {commands:?}"
    );
    found[0]
}

/// `task sherpa-onnx-tts` fetches the two archives from their fixed release URLs over https,
/// checks each against its pin before it is moved into place and unpacked, and unpacks below its
/// own directory under `target/`. The pins are the adapter's and the story's.
#[test]
fn the_fetch_task_checks_each_archive_against_its_pin_before_it_unpacks_it() {
    let taskfile = read(&repository().join("Taskfile.yml"));
    let lines = task(&taskfile, "sherpa-onnx-tts");
    assert_eq!(variable(&lines, "CACHE"), "target/sherpa-onnx-tts");
    let native = variable(&lines, "NATIVE");
    assert_eq!(
        native,
        format!("sherpa-onnx-v{NATIVE_VERSION}-linux-x64-shared-lib")
    );
    assert_eq!(
        variable(&lines, "NATIVE_URL"),
        format!(
            "https://github.com/k2-fsa/sherpa-onnx/releases/download/v{NATIVE_VERSION}/{native}.tar.bz2"
        )
    );
    assert_eq!(variable(&lines, "NATIVE_SHA256"), NATIVE_ARCHIVE_SHA256);
    assert_eq!(variable(&lines, "CHECKPOINT"), CHECKPOINT);
    assert_eq!(variable(&lines, "CHECKPOINT_URL"), ARCHIVE_URL);
    assert!(ARCHIVE_URL.starts_with("https://github.com/k2-fsa/sherpa-onnx/releases/download/"));
    assert_eq!(variable(&lines, "CHECKPOINT_SHA256"), ARCHIVE_SHA256);

    let commands: Vec<&str> = lines
        .iter()
        .skip_while(|line| **line != "cmds:")
        .skip(1)
        .map(|line| line.strip_prefix("- ").expect("a command of the task"))
        .collect();
    for (archive, unpacked) in [("NATIVE", "native"), ("CHECKPOINT", "checkpoint")] {
        let file = format!("{{{{.CACHE}}}}/{{{{.{archive}}}}}.tar.bz2");
        let fetched = the_one(&commands, "curl ", &format!("--output {file}.part "));
        assert!(
            commands[fetched].contains("--fail")
                && commands[fetched].ends_with(&format!(" {{{{.{archive}_URL}}}}")),
            "{}",
            commands[fetched]
        );
        let checked = the_one(
            &commands,
            "echo ",
            &format!("\"{{{{.{archive}_SHA256}}}}  {file}.part\" | sha256sum --check --strict"),
        );
        let moved = the_one(&commands, "mv ", &format!("{file}.part {file}"));
        let unpacked = the_one(
            &commands,
            "tar ",
            &format!("-xjf {file} -C {{{{.CACHE}}}}/{unpacked}"),
        );
        assert!(
            fetched < checked && checked < moved && moved < unpacked,
            "{archive} is fetched, checked, moved into place and only then unpacked: {commands:?}"
        );
    }
    assert_eq!(
        commands
            .iter()
            .filter(|command| command.starts_with("tar ") || command.starts_with("curl "))
            .count(),
        4,
        "two archives are fetched and two unpacked: {commands:?}"
    );
}

/// No other task and no workflow of the forge runs the fetch task, so the library with the
/// GPL-licensed phonemiser reaches a machine only by a person's own command.
#[test]
fn no_other_task_and_no_workflow_runs_the_fetch_task() {
    let repository = repository();
    let taskfile = read(&repository.join("Taskfile.yml"));
    let callers: Vec<&str> = taskfile
        .lines()
        .filter(|line| {
            let line = line.trim();
            line.ends_with("task: sherpa-onnx-tts")
                || line.contains("task sherpa-onnx-tts")
                || line.contains("sherpa-onnx-tts }")
        })
        .collect();
    assert!(callers.is_empty(), "{callers:?}");

    let workflows = repository.join(".github/workflows");
    let entries =
        fs::read_dir(&workflows).unwrap_or_else(|error| panic!("{}: {error}", workflows.display()));
    let mut read_any = false;
    for entry in entries {
        let path = entry.expect("the entry is read").path();
        let workflow = read(&path);
        read_any = true;
        assert!(
            !workflow.contains("sherpa-onnx-tts") && !workflow.contains("shared-lib"),
            "{} names the fetch task or the archive with text-to-speech",
            path.display()
        );
    }
    assert!(read_any, "the workflows are read");
}
