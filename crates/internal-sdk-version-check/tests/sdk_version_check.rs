//! Tests for the SDK version check. They drive the public API and the check binary only.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use internal_sdk_version_check::{check_sdk_version, published_version};

/// A version no package publishes, so a README that states it is wrong.
const OTHER_VERSION: &str = "9.9.9";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("the repository root")
}

fn package_manifest() -> String {
    fs::read_to_string(repo_root().join("packages/aether-sdk/package.json")).expect("the package manifest reads")
}

fn readme_path() -> PathBuf {
    repo_root().join("README.md")
}

fn sdk_readme_path() -> PathBuf {
    repo_root().join("packages/aether-sdk/README.md")
}

fn readme(path: &Path) -> String {
    fs::read_to_string(path).expect("the README reads")
}

/// The SDK README with the published version replaced by one the package does not publish.
fn sdk_readme_with_another_version() -> String {
    let published = published_version(&package_manifest()).expect("the package manifest holds a version");
    readme(&sdk_readme_path()).replace(&published, OTHER_VERSION)
}

fn run_check(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_sdk-version-check")).arg(root).output().expect("the check runs")
}

#[test]
fn the_current_readmes_state_the_published_sdk_version() {
    let manifest = package_manifest();
    let published = published_version(&manifest).expect("the package manifest holds a version");

    assert!(!published.is_empty(), "the package manifest holds a version");

    for path in [readme_path(), sdk_readme_path()] {
        let report = check_sdk_version(&manifest, &readme(&path)).expect("the README states a version");

        assert!(
            report.is_clean(),
            "{} states version {} but the package publishes version {}",
            path.display(),
            report.stated,
            report.published
        );
        assert_eq!(report.published, published, "{}", path.display());
    }
}

#[test]
fn a_readme_that_states_another_version_is_reported() {
    let manifest = package_manifest();
    let published = published_version(&manifest).expect("the package manifest holds a version");

    let report = check_sdk_version(&manifest, &sdk_readme_with_another_version()).expect("the README states a version");

    let mismatch = report.mismatch().expect("the check reports the difference");
    assert_eq!(mismatch.published, published);
    assert_eq!(mismatch.stated, OTHER_VERSION);
    let message = mismatch.to_string();
    assert!(message.contains(&published), "the failure must name the published version: {message}");
    assert!(message.contains(OTHER_VERSION), "the failure must name the stated version: {message}");
}

#[test]
fn the_check_fails_on_a_mismatched_version() {
    let published = published_version(&package_manifest()).expect("the package manifest holds a version");
    let directory = tempfile::tempdir().expect("a temporary directory");
    let sdk_directory = directory.path().join("packages/aether-sdk");
    fs::create_dir_all(&sdk_directory).expect("the package directory is created");
    fs::write(sdk_directory.join("package.json"), package_manifest()).expect("the package manifest writes");
    fs::write(sdk_directory.join("README.md"), sdk_readme_with_another_version()).expect("the README writes");
    fs::write(directory.path().join("README.md"), readme(&readme_path())).expect("the README writes");

    let output = run_check(directory.path());

    assert!(!output.status.success(), "the check must fail when a README states another version");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains(&published), "the failure must name the published version: {stderr}");
    assert!(stderr.contains(OTHER_VERSION), "the failure must name the stated version: {stderr}");
}

#[test]
fn the_check_passes_on_the_current_workspace() {
    let output = run_check(&repo_root());

    assert!(
        output.status.success(),
        "the check must pass on the current workspace: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
