//! Cases of the adversary pass on `adopt-records`: inputs the program should refuse and writes it
//! should not make. Its second plant of each rule that a narrowed copy of a rule would miss stands
//! in `PLANTS` of the module above.

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};

use super::{COMMIT, Case, RECORDS, RUN, TEMPORARY};

/// A record whose text holds one key twice is refused: the program keeps the last value, a
/// reader that keeps the first one reads another project from the same bytes.
#[test]
fn a_record_that_holds_a_key_twice_is_refused() {
    let case = Case::new();
    case.edit(
        "adapters/aws-records.evidence.json",
        "\"project\": \"inference\",",
        "\"project\": \"elsewhere\",\n    \"project\": \"inference\",",
    );
    let outcome = case.adopt();
    case.refused(&outcome, &[("adapters/aws-records.evidence.json", "")]);
}

/// A link standing in the place of a temporary file is not written through: the file it names,
/// outside the records, keeps its bytes, and the record is no link afterwards.
#[test]
fn a_link_in_the_place_of_a_temporary_file_is_not_written_through() {
    let case = Case::new();
    let outside = case.directory.join("outside.txt");
    fs::write(&outside, b"outside\n").unwrap();
    let records = case.root().join(RECORDS);
    symlink(
        &outside,
        records.join(format!(".aws-records.evidence.json{TEMPORARY}")),
    )
    .unwrap();
    let outcome = case.adopt();
    assert_eq!(
        fs::read(&outside).unwrap(),
        b"outside\n",
        "the file the link names was written; status {:?}, stderr: {}",
        outcome.status,
        outcome.stderr
    );
    let record = records.join("aws-records.evidence.json");
    assert!(
        !fs::symlink_metadata(&record)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the record is a link"
    );
}

/// A target that `assess` never writes is refused: a revision target that also names a file,
/// and a document target whose digest is of another form than the bytes.
#[test]
fn a_target_that_assess_never_writes_is_refused() {
    let case = Case::new();
    case.edit(
        "adapters/aws-records.evidence.json",
        &format!("\"revision\": \"{COMMIT}\""),
        &format!(
            "\"revision\": \"{COMMIT}\",\n      \"identifiers\": {{\"path\": \"Cargo.toml\", \"digest\": \"sha256:{}\", \"digest_form\": \"bytes\"}}",
            "0".repeat(64)
        ),
    );
    case.edit(
        "adapters/sherpa-onnx-stt-record.evidence.json",
        "\"digest_form\": \"bytes\"",
        "\"digest_form\": \"text\"",
    );
    let outcome = case.adopt();
    case.refused(
        &outcome,
        &[
            ("adapters/aws-records.evidence.json", ""),
            ("adapters/sherpa-onnx-stt-record.evidence.json", ""),
        ],
    );
}

/// The descriptions of a run of the adversary brief that should be refused: `main` in another
/// form, another event, a field left out.
#[test]
fn the_run_descriptions_of_the_brief_are_refused() {
    for (from, to, line) in [
        (
            "\"headBranch\":\"main\"",
            "\"headBranch\":\"refs/heads/main\"",
            "headBranch is Some(\"refs/heads/main\")",
        ),
        (
            "\"headBranch\":\"main\"",
            "\"headBranch\":\"main \"",
            "headBranch is Some(\"main \")",
        ),
        (
            "\"event\":\"push\"",
            "\"event\":\"pull_request\"",
            "event is Some(\"pull_request\")",
        ),
        (",\"workflowName\":\"check\"", "", "workflowName is None"),
        (
            format!("\"databaseId\":{RUN},").as_str(),
            "",
            "with no databaseId",
        ),
    ] {
        let case = Case::new();
        case.edit_run(from, to);
        let outcome = case.adopt();
        case.refused(&outcome, &[("run.json", line)]);
    }
}

/// The artifacts and the plan of the adversary brief that should be refused, each refused naming
/// its file.
#[test]
fn the_inputs_of_the_brief_are_refused() {
    let case = Case::new();
    let artifact = case.artifact();
    let linked = artifact.join("gate/workspace-tests.evidence.json");
    let outside = case.directory.join("outside.json");
    fs::rename(&linked, &outside).unwrap();
    symlink(&outside, &linked).unwrap();
    fs::rename(
        artifact.join("adapters/aws-records.evidence.json"),
        artifact.join("adapters/AWS-records.evidence.json"),
    )
    .unwrap();
    fs::copy(
        artifact.join("gate/release-statuses.evidence.json"),
        artifact.join("adapters/release-statuses.evidence.json"),
    )
    .unwrap();
    let trailing = format!(
        "{}\n{{}}\n",
        case.record("adapters/cartesia-records.evidence.json")
    );
    fs::write(
        artifact.join("adapters/cartesia-records.evidence.json"),
        trailing,
    )
    .unwrap();
    case.edit(
        "adapters/deepgram-records.evidence.json",
        "github.com/beyond10x/inference/actions",
        "github.com/beyond10x/other/actions",
    );
    case.edit(
        "adapters/openai-records.evidence.json",
        "https://github.com",
        "http://github.com",
    );
    let outcome = case.adopt();
    let extra = "is no record of an assessment";
    case.refused(
        &outcome,
        &[
            ("gate/workspace-tests.evidence.json", extra),
            ("gate/workspace-tests.evidence.json", "is missing"),
            ("adapters/AWS-records.evidence.json", extra),
            ("adapters/aws-records.evidence.json", "is missing"),
            ("adapters/release-statuses.evidence.json", extra),
            (
                "adapters/cartesia-records.evidence.json",
                "is not valid JSON",
            ),
            (
                "adapters/deepgram-records.evidence.json",
                &format!("not the run {RUN}"),
            ),
            (
                "adapters/openai-records.evidence.json",
                &format!("not the run {RUN}"),
            ),
        ],
    );

    let case = Case::new();
    let plan = case.root().join("quality/adapters.ratchet.yaml");
    let text = fs::read_to_string(&plan).unwrap();
    let test = "the_nova_sonic_record_holds_the_claims_of_the_binding";
    assert!(text.contains(test), "the plan lists {test}");
    fs::write(&plan, text.replacen(test, "a_test_the_run_did_not_run", 1)).unwrap();
    let outcome = case.adopt();
    case.refused(
        &outcome,
        &[
            (
                "adapters/aws-records.evidence.json",
                "which assessments.aws-records lists",
            ),
            (
                "adapters/aws-records.evidence.json",
                "which assessments.aws-records does not list",
            ),
        ],
    );
}

/// A records directory the program cannot write to ends with exit 3 and leaves every record.
#[test]
fn a_records_directory_that_is_read_only_leaves_every_record() {
    let case = Case::new();
    let records = case.root().join(RECORDS);
    fs::set_permissions(&records, fs::Permissions::from_mode(0o555)).unwrap();
    let probe = records.join(".probe");
    if fs::write(&probe, b"").is_ok() {
        let _ = fs::remove_file(&probe);
        fs::set_permissions(&records, fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let outcome = case.adopt();
    fs::set_permissions(&records, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(outcome.status, Some(3), "stderr: {}", outcome.stderr);
    case.untouched(&[]);
}

/// What a run killed between two renames leaves: some records adopted, the temporary files of
/// the others. A second run on the same artifact adopts all of them and leaves no temporary file.
#[test]
fn a_second_run_after_a_kill_between_two_renames_adopts_every_record() {
    let case = Case::new();
    let records = case.root().join(RECORDS);
    let mut paths: Vec<String> = Vec::new();
    for plan in ["adapters", "gate"] {
        for entry in fs::read_dir(case.artifact().join(plan)).unwrap() {
            let name = entry.unwrap().file_name().into_string().unwrap();
            paths.push(format!("{plan}/{name}"));
        }
    }
    paths.sort();
    for (at, path) in paths.iter().enumerate() {
        let bytes = fs::read(case.artifact().join(path)).unwrap();
        let name = path.rsplit('/').next().unwrap();
        if at % 2 == 0 {
            fs::write(records.join(name), &bytes).unwrap();
        } else {
            fs::write(records.join(format!(".{name}{TEMPORARY}")), &bytes).unwrap();
        }
    }
    let outcome = case.adopt();
    assert_eq!(outcome.status, Some(0), "stderr: {}", outcome.stderr);
    for path in &paths {
        let name = path.rsplit('/').next().unwrap();
        assert_eq!(
            fs::read(records.join(name)).unwrap(),
            fs::read(case.artifact().join(path)).unwrap(),
            "{name} is the record of the artifact"
        );
    }
    let left: Vec<String> = fs::read_dir(&records)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(TEMPORARY))
        .collect();
    assert!(left.is_empty(), "temporary files are left: {left:?}");
}
