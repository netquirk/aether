//! Tests for the private-item lint. They drive the public API and the lint binary only.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use internal_lints::{PrivateItemKind, scan_workspace_private};

/// The root of this crate: the directory that holds its `Cargo.toml`.
///
/// Cargo runs an integration test with the package root as the current
/// directory. The current directory is preferred over the compile-time
/// manifest directory, so that a checkout which is moved or removed after the
/// test binary was built still finds the fixtures.
fn crate_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let current = std::env::current_dir().unwrap_or_else(|_| manifest.clone());
    if current.join("Cargo.toml").is_file() { current } else { manifest }
}

fn repo_root() -> PathBuf {
    crate_root().join("../..").canonicalize().expect("the repository root")
}

fn fixture_root() -> PathBuf {
    crate_root().join("tests/fixtures/private-crate")
}

fn public_fixture_root() -> PathBuf {
    crate_root().join("tests/fixtures/public-crate")
}

fn module_fixture_root() -> PathBuf {
    crate_root().join("tests/fixtures/module-crate")
}

fn importing_fixture_root() -> PathBuf {
    crate_root().join("tests/fixtures/importing-crate")
}

fn lint() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_private-lint"));
    command.current_dir(repo_root());
    command
}

fn run_lint(path: &Path) -> Output {
    lint().arg(path).output().expect("the lint runs")
}

/// The line the file holds the expression on.
fn line_of(path: &Path, expression: &str) -> usize {
    let source = fs::read_to_string(path).expect("the fixture reads");
    source
        .lines()
        .position(|line| line.contains(expression))
        .map_or_else(|| panic!("{} holds no {expression}", path.display()), |index| index + 1)
}

#[test]
fn cli_fails_naming_the_file_the_line_and_the_private_item() {
    let output = run_lint(&fixture_root());
    assert!(!output.status.success(), "the lint must fail when a test reaches a private item");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stdout.contains("scanned 4 test functions"),
        "the lint must print how many test functions it scanned: {stdout}"
    );
    assert!(stdout.contains("flagged 4 test functions"), "the lint must print how many it flagged: {stdout}");
    assert!(stderr.contains("src/lib.rs:"), "the finding must name the file: {stderr}");
    assert!(stderr.contains("calls_the_private_function"), "the finding must name the test function: {stderr}");
    assert!(stderr.contains("private_helper"), "the finding must name the private item: {stderr}");
}

#[test]
fn private_call_is_reported_with_its_file_and_line() {
    let report = scan_workspace_private(&fixture_root());
    let finding = report
        .findings
        .iter()
        .find(|finding| finding.test == "calls_the_private_function")
        .expect("the private call is reported");
    assert_eq!(finding.kind, PrivateItemKind::Function);
    assert_eq!(finding.item, "private_helper");
    assert!(finding.path.ends_with("src/lib.rs"), "the finding must name the file: {:?}", finding.path);
    let expected = line_of(&fixture_root().join("src/lib.rs"), "assert_eq!(private_helper(), 42)");
    assert_eq!(finding.line, expected, "the finding must name the line of the call");
    assert!(finding.detail.contains("private_helper()"), "the finding must quote the call: {}", finding.detail);
}

#[test]
fn private_field_is_reported_with_its_file_and_line() {
    let report = scan_workspace_private(&fixture_root());
    let finding = report
        .findings
        .iter()
        .find(|finding| {
            finding.kind == PrivateItemKind::Field
                && finding.test == "reaches_the_private_field"
                && finding.detail.contains("config.secret")
        })
        .expect("the private field is reported");
    assert_eq!(finding.item, "secret");
    assert!(finding.path.ends_with("src/lib.rs"), "the finding must name the file: {:?}", finding.path);
    let expected = line_of(&fixture_root().join("src/lib.rs"), "assert_eq!(config.secret, 7)");
    assert_eq!(finding.line, expected, "the finding must name the line of the field access");
}

#[test]
fn private_method_is_reported_by_name() {
    let report = scan_workspace_private(&fixture_root());
    let finding = report
        .findings
        .iter()
        .find(|finding| finding.kind == PrivateItemKind::Method)
        .expect("the private method is reported");
    assert_eq!(finding.item, "private_method");
    assert_eq!(finding.test, "calls_the_private_method");
}

#[test]
fn integration_test_reaching_a_private_item_is_reported() {
    let report = scan_workspace_private(&fixture_root());
    let finding = report
        .findings
        .iter()
        .find(|finding| finding.test == "calls_the_private_function_from_outside")
        .expect("the private reach of the integration test is reported");
    assert_eq!(finding.kind, PrivateItemKind::Function);
    assert!(finding.path.ends_with("tests/reaches_private.rs"), "the finding must name the file: {:?}", finding.path);
    assert_eq!(report.tests_flagged, 4, "the fixture holds four tests the lint reports");
}

#[test]
fn integration_test_importing_a_private_item_is_reported() {
    let report = scan_workspace_private(&importing_fixture_root());
    let finding = report
        .findings
        .iter()
        .find(|finding| finding.test == "imports_the_private_function")
        .expect("the import of the private item is reported");
    assert_eq!(finding.kind, PrivateItemKind::Function);
    assert_eq!(finding.item, "private_helper");
    assert!(finding.path.ends_with("tests/imports_private.rs"), "the finding must name the file: {:?}", finding.path);
    let expected = line_of(
        &importing_fixture_root().join("tests/imports_private.rs"),
        "use importing_crate_fixture::private_helper;",
    );
    assert_eq!(finding.line, expected, "the finding must name the line of the import");
    assert!(
        finding.detail.contains("use importing_crate_fixture::private_helper;"),
        "the finding must quote the import: {}",
        finding.detail
    );
}

#[test]
fn unit_test_importing_a_private_item_of_the_crate_root_is_reported() {
    let report = scan_workspace_private(&importing_fixture_root());
    let finding = report
        .findings
        .iter()
        .find(|finding| finding.test == "imports_the_private_function_from_inside")
        .expect("the import of the private item is reported");
    assert_eq!(finding.kind, PrivateItemKind::Function);
    assert_eq!(finding.item, "private_helper");
    assert!(finding.path.ends_with("src/lib.rs"), "the finding must name the file: {:?}", finding.path);
    let expected = line_of(&importing_fixture_root().join("src/lib.rs"), "use crate::private_helper;");
    assert_eq!(finding.line, expected, "the finding must name the line of the import");
    assert!(
        finding.detail.contains("use crate::private_helper;"),
        "the finding must quote the import: {}",
        finding.detail
    );
}

#[test]
fn the_private_items_of_the_module_a_test_is_written_in_are_not_reported() {
    let report = scan_workspace_private(&module_fixture_root());
    let reported: Vec<&str> = report.findings.iter().map(|finding| finding.test.as_str()).collect();
    assert!(
        !reported.contains(&"reaches_the_private_helper_of_the_module_it_is_written_in"),
        "the private items of the module a test is written in are not reported: {reported:?}"
    );
    assert!(
        !reported.contains(&"reaches_the_private_helper_of_the_module_it_is_nested_in"),
        "the private items of the modules a test is nested in are not reported: {reported:?}"
    );
    assert!(
        reported.contains(&"reaches_the_private_helper_of_the_crate_root"),
        "the private items of the crate root are reported: {reported:?}"
    );
    assert!(
        reported.contains(&"reaches_the_private_helper_of_a_sibling_module"),
        "the private items of another module are reported: {reported:?}"
    );
    assert_eq!(report.tests_scanned, 4, "the fixture holds four test functions");
    assert_eq!(report.tests_flagged, 2, "the lint reports two of the four");
}

#[test]
fn public_fixture_is_clean() {
    let report = scan_workspace_private(&public_fixture_root());
    assert!(report.findings.is_empty(), "the lint must leave the public fixture alone: {:?}", report.findings);
    assert!(report.problems.is_empty(), "every file of the fixture must parse: {:?}", report.problems);
    assert_eq!(report.tests_scanned, 3, "the fixture holds three test functions");
    assert_eq!(report.tests_flagged, 0, "the fixture holds no test the lint reports");
}

#[test]
fn workspace_scan_finds_no_private_reach() {
    let report = scan_workspace_private(&repo_root());
    assert!(report.problems.is_empty(), "every file of the workspace must parse: {:?}", report.problems);
    assert!(report.findings.is_empty(), "no test of the workspace reaches a private item: {:?}", report.findings);
    assert!(
        report.tests_scanned > 1000,
        "the workspace holds thousands of test functions, got {}",
        report.tests_scanned
    );
}

#[test]
fn private_lint_runs_with_a_scrubbed_environment() {
    let mut command = Command::new(env!("CARGO_BIN_EXE_private-lint"));
    command.current_dir(repo_root()).env_clear();
    let output = command.output().expect("the lint runs");
    assert!(
        output.status.success(),
        "the lint must run without a credential or a network: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("scanned"), "the lint must report what it scanned: {stdout}");
}
