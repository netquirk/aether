//! Tests for the error-handling lint. They drive the public API and the lint binary only.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use internal_lints::{Finding, ForbiddenKind, scan_crate, scan_workspace};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().expect("the repository root")
}

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/anyhow-crate")
}

fn lint() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_internal-lints"));
    command.current_dir(repo_root());
    command
}

fn run_lint(path: &Path) -> Output {
    lint().arg(path).output().expect("the lint runs")
}

fn workspace_crates(root: &Path) -> Vec<String> {
    let manifest = fs::read_to_string(root.join("Cargo.toml")).expect("the workspace manifest");
    let value = manifest.parse::<toml::Value>().expect("the workspace manifest parses");
    let members = value
        .get("workspace")
        .and_then(|workspace| workspace.get("members"))
        .and_then(toml::Value::as_array)
        .expect("the workspace members");
    let mut crates = Vec::new();
    for member in members {
        let member = member.as_str().expect("a member path");
        let prefix = member.trim_end_matches('*').trim_end_matches('/');
        for entry in fs::read_dir(root.join(prefix)).expect("the crates directory") {
            let path = entry.expect("a crate directory").path();
            if path.join("Cargo.toml").is_file() {
                crates.push(path.display().to_string());
            }
        }
    }
    crates
}

fn workspace_packages(root: &Path) -> Vec<String> {
    let mut packages = Vec::new();
    for entry in fs::read_dir(root.join("packages")).expect("the packages directory") {
        let path = entry.expect("a package directory").path();
        if path.join("package.json").is_file() {
            packages.push(path.display().to_string());
        }
    }
    packages
}

#[test]
fn workspace_scan_covers_every_crate_and_package() {
    let root = repo_root();
    let report = scan_workspace(&root);
    let scanned: Vec<String> = report.scanned.iter().map(|path| path.display().to_string()).collect();
    for crate_dir in workspace_crates(&root) {
        assert!(scanned.iter().any(|path| path.starts_with(&crate_dir)), "no file of {crate_dir} was scanned");
    }
    for package_dir in workspace_packages(&root) {
        assert!(scanned.iter().any(|path| path.starts_with(&package_dir)), "no file of {package_dir} was scanned");
    }
}

#[test]
fn cli_fails_with_counts_per_item_kind() {
    let output = run_lint(&fixture_root());
    assert!(!output.status.success(), "the lint must fail when it finds a forbidden item");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    for kind in ForbiddenKind::all() {
        assert!(stdout.contains(&format!("{}: ", kind.label())), "the counts must name {}: {stdout}", kind.label());
    }
    assert!(stderr.contains("src/lib.rs:1:"), "the finding must name the file and the line: {stderr}");
    assert!(stderr.contains(": anyhow crate:"), "the finding must name the forbidden item: {stderr}");
}

#[test]
fn anyhow_fixture_crate_is_reported_with_file_and_line() {
    let report = scan_crate(&fixture_root());
    let usages: Vec<&Finding> =
        report.findings.iter().filter(|finding| finding.kind == ForbiddenKind::AnyhowCrate).collect();
    let in_source =
        usages.iter().find(|finding| finding.path.ends_with("src/lib.rs")).expect("the fixture uses anyhow::Result");
    assert_eq!(in_source.line, 1, "the finding must name the line of the anyhow use");
    assert!(in_source.detail.contains("anyhow"), "the finding must name the forbidden item");
    assert!(usages.iter().any(|finding| finding.path.ends_with("Cargo.toml")), "the fixture depends on anyhow");
}

#[test]
fn lint_runs_with_a_scrubbed_environment() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_internal-lints"));
    command.current_dir(repo_root()).env_clear();
    let output = command.output().expect("the lint runs");
    assert!(
        output.status.success(),
        "the lint must run without a credential or a network: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn cli_prints_the_paths_it_scanned() {
    let output = run_lint(&repo_root());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("crates/aether-cli/src/lib.rs"),
        "the lint must print the crate paths it scanned: {stdout}"
    );
    assert!(
        stdout.contains("packages/aether-sdk/package.json"),
        "the lint must print the package paths it scanned: {stdout}"
    );
}

#[test]
fn workspace_scan_ignores_the_nested_fixture_crate() {
    let report = scan_workspace(&repo_root());
    assert!(report.is_clean(), "the repository must obey the rule: {:?}", report.findings);
    assert!(
        !report.scanned.iter().any(|path| path.to_string_lossy().contains("anyhow-crate")),
        "the fixture crate is a package of its own, not part of the workspace scan"
    );
}
