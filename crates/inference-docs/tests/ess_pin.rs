use std::fs;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn ci_ess_version() -> String {
    let workflow = fs::read_to_string(root().join(".github/workflows/check.yml")).unwrap();
    let line = workflow
        .lines()
        .find(|line| line.trim_start().starts_with("ESS_VERSION:"))
        .expect("check.yml sets ESS_VERSION");
    line.split(':')
        .nth(1)
        .unwrap()
        .trim()
        .trim_matches('"')
        .to_owned()
}

#[test]
fn the_run_the_checks_guide_names_the_ess_version_ci_installs() {
    let version = ci_ess_version();
    let guide = fs::read_to_string(root().join("website/docs/guides/run-the-checks.md")).unwrap();
    let claim = format!("With ess {version}, the release CI installs");
    assert!(
        guide.contains(&claim),
        "website/docs/guides/run-the-checks.md does not say {claim:?}; CI installs ess {version}"
    );
    assert!(
        !guide.contains("which is newer"),
        "website/docs/guides/run-the-checks.md shows a 'newer' warning that ess {version} no longer prints for ess/ess-inputs.yaml"
    );
}
